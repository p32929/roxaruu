//! Tab strips that hold more tabs than fit: they scroll sideways (wheel,
//! trackpad, a finger, or the "‹ N" / "N ›" ends, which bring the next
//! hidden tab in), keep the active tab in view, and let a tab be
//! dragged to a new place. Shared by the editor's tabs and the terminals'.

use super::*;

/// A strip's scroll and drag, per view (the window and a browser differ in
/// width).
#[derive(Default, Clone, Copy)]
pub struct Strip {
    /// How far it's scrolled, in points.
    pub offset: f32,
    /// The active tab last frame: a new one is scrolled into view.
    shown: Option<usize>,
    /// Keep the active tab in view as tabs change width (a terminal's name
    /// growing as its program starts, tabs opening, the window resizing),
    /// until the strip is scrolled by hand.
    follow: bool,
    pub drag: Option<Drag>,
}

/// A tab held by the mouse: which, where in it it was grabbed, and where it
/// would land now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub from: usize,
    grab: f32,
    pub to: usize,
}

/// Where each tab goes this frame.
pub(super) struct Layout {
    /// By tab index (the dragged one follows the pointer).
    pub rects: Vec<Rect>,
    pub hidden_left: usize,
    pub hidden_right: usize,
    /// A drag just ended: move tab `.0` to `.1`.
    pub moved: Option<(usize, usize)>,
}

impl Strip {
    /// Take the scrolling over `room`, keep `active` in view (clear of the
    /// `end` markers, as wide as they are), follow a drag, and place tabs of
    /// `widths` (by index) `gap` apart.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn layout(
        &mut self,
        ui: &egui::Ui,
        room: Rect,
        widths: &[f32],
        gap: f32,
        end: f32,
        active: Option<usize>,
    ) -> Layout {
        let n = widths.len();
        let starts = starts(widths, gap);
        let total = starts.last().zip(widths.last()).map_or(0.0, |(s, w)| s + w);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        if pointer.is_some_and(|p| room.contains(p)) {
            // Up and down on a wheel go sideways here.
            let d = ui.input(|i| i.smooth_scroll_delta);
            if d != egui::Vec2::ZERO {
                self.offset -= d.x + d.y;
                self.follow = false;
            }
        }
        if active != self.shown {
            self.shown = active;
            self.follow = true;
        }
        if self.follow
            && self.drag.is_none()
            && let Some(a) = active.filter(|a| *a < n)
            && let (Some(&s), Some(&w)) = (starts.get(a), widths.get(a))
        {
            // With tabs past it on a side, that side's end marker covers
            // the strip's edge: clear it too.
            let left = s - if a > 0 { end } else { 0.0 };
            let right = s + w + if a + 1 < n { end } else { 0.0 };
            self.offset = reveal(self.offset, left, right, room.width());
        }
        let mut moved = None;
        if let Some(d) = &mut self.drag {
            let down = ui.input(|i| i.pointer.primary_down());
            match pointer.filter(|_| down && d.from < n) {
                Some(p) => {
                    // Its middle, along the unscrolled strip, says where it lands.
                    let mid = p.x - d.grab + widths.get(d.from).copied().unwrap_or(0.0) / 2.0 - room.left() + self.offset;
                    d.to = drop_index(widths, gap, d.from, mid);
                    // Held near an end: the strip scrolls on.
                    let edge = 28.0;
                    let push = if p.x < room.left() + edge {
                        -1.0
                    } else if p.x > room.right() - edge {
                        1.0
                    } else {
                        0.0
                    };
                    if push != 0.0 {
                        self.offset += push * 8.0;
                        ui.ctx().request_repaint();
                    }
                }
                None => {
                    moved = (d.to != d.from && d.from < n).then_some((d.from, d.to));
                    self.drag = None;
                    self.shown = None; // the moved tab, if active, stays in view
                }
            }
        }
        self.offset = self.offset.clamp(0.0, (total - room.width()).max(0.0));

        // Lay them out in the order they'd have if dropped now.
        let order = match self.drag {
            Some(d) => moved_order(n, d.from, d.to),
            None => (0..n).collect(),
        };
        let mut rects = vec![Rect::NOTHING; n];
        let mut x = room.left() - self.offset;
        for i in order {
            let w = widths.get(i).copied().unwrap_or(0.0);
            if let Some(r) = rects.get_mut(i) {
                *r = Rect::from_min_size(pos2(x, room.top()), vec2(w, room.height()));
            }
            x += w + gap;
        }
        if let (Some(d), Some(p)) = (self.drag, pointer)
            && let (Some(&w), Some(r)) = (widths.get(d.from), rects.get_mut(d.from))
        {
            let left = (p.x - d.grab).clamp(room.left(), (room.right() - w).max(room.left()));
            *r = Rect::from_min_size(pos2(left, room.top()), vec2(w, room.height()));
        }
        #[cfg(test)]
        LAID.with(|l| l.borrow_mut().push((room, rects.clone())));
        let hidden_left = rects.iter().filter(|r| r.left() < room.left() - 1.0).count();
        let hidden_right = rects.iter().filter(|r| r.right() > room.right() + 1.0).count();
        Layout { rects, hidden_left, hidden_right, moved }
    }

    /// Bring the active tab into view again, even if it's the same one
    /// (chosen again after the strip was scrolled away from it).
    pub fn reveal_again(&mut self) {
        self.shown = None;
    }

    /// How much of the strip an end marker ("‹ N" and its fade) covers.
    pub(super) fn end_width(s: f32) -> f32 {
        72.0 * s
    }

    /// A tab's press turned into a drag.
    pub(super) fn start_drag(&mut self, i: usize, grab: f32) {
        self.drag = Some(Drag { from: i, grab, to: i });
    }

    /// The "‹ N" / "N ›" ends over a strip that doesn't fit: how many tabs
    /// are past each end; a click brings the next one in.
    pub(super) fn ends(&mut self, ui: &egui::Ui, pt: &Paint, room: Rect, lay: &Layout, keys: [String; 2]) {
        let (c, s) = (&pt.c, pt.s);
        let p = ui.painter();
        for (left, count) in [(true, lay.hidden_left), (false, lay.hidden_right)] {
            if count == 0 {
                continue;
            }
            let text = if left {
                format!("{} {count}", icon::CHEVRON_LEFT)
            } else {
                format!("{count} {}", icon::CHEVRON_RIGHT)
            };
            let g = ui.fonts_mut(|f| f.layout_no_wrap(text, pt.font(12.0, Weight::Medium), c.text_subtle));
            let w = g.size().x + 14.0 * s;
            let r = if left {
                Rect::from_min_size(room.left_top(), vec2(w, room.height()))
            } else {
                Rect::from_min_size(pos2(room.right() - w, room.top()), vec2(w, room.height()))
            };
            // A fade into the strip, so tabs read as going on under it.
            let fade = 18.0 * s;
            let (from, to) = if left { (r.right(), r.right() + fade) } else { (r.left() - fade, r.left()) };
            for k in 0..6 {
                let a = (k as f32 + 0.5) / 6.0;
                let x0 = from + (to - from) * k as f32 / 6.0;
                let band = Rect::from_x_y_ranges(x0..=x0 + (to - from) / 6.0 + 0.5, r.y_range());
                let alpha = if left { 1.0 - a } else { a };
                p.rect_filled(band, 0.0, c.surface.gamma_multiply(alpha));
            }
            let resp = ui.interact(r, ui.scope_id().with(("strip-end", left)), Sense::click());
            p.rect_filled(r, 0.0, if resp.hovered() { mix(c.surface, c.hover, 1.0) } else { c.surface });
            p.galley(pos2(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0), g, c.text_subtle);
            let side = if left { "left" } else { "right" };
            let [to_left, to_right] = &keys;
            let key = if left { to_left } else { to_right };
            let tip = format!("{count} more to the {side} · click, scroll or {key} to see them");
            if resp.on_hover_text(tip).clicked() {
                // The first tab cut off at that end comes fully in.
                let hidden = lay.rects.iter().filter(|t| {
                    if left { t.left() < room.left() - 1.0 } else { t.right() > room.right() + 1.0 }
                });
                let next = if left {
                    hidden.max_by(|a, b| a.left().total_cmp(&b.left()))
                } else {
                    hidden.min_by(|a, b| a.left().total_cmp(&b.left()))
                };
                if let Some(t) = next {
                    self.follow = false; // looking around, by hand
                    // Past the end marker too.
                    self.offset += if left { t.left() - room.left() - w } else { t.right() - room.right() + w };
                    ui.ctx().request_repaint();
                }
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    static LAID: std::cell::RefCell<Vec<(Rect, Vec<Rect>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test-only: every strip laid out since last asked (its room, its tabs).
#[cfg(test)]
pub fn laid_out() -> Vec<(Rect, Vec<Rect>)> {
    LAID.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

/// Where each tab starts along the unscrolled strip.
fn starts(widths: &[f32], gap: f32) -> Vec<f32> {
    widths
        .iter()
        .scan(0.0, |x, w| {
            let at = *x;
            *x += w + gap;
            Some(at)
        })
        .collect()
}

/// The scroll that shows `left..right` in a strip `room` wide, moving as
/// little as it can.
fn reveal(offset: f32, left: f32, right: f32, room: f32) -> f32 {
    if left < offset {
        left
    } else if right > offset + room {
        (right - room).min(left)
    } else {
        offset
    }
}

/// Where the dragged tab `from` lands with its middle at `mid` (along the
/// unscrolled strip): before the first other tab whose middle, where it
/// stood, is past it (so picking a tab up moves nothing).
fn drop_index(widths: &[f32], gap: f32, from: usize, mid: f32) -> usize {
    let starts = starts(widths, gap);
    (0..widths.len())
        .filter(|i| *i != from)
        .position(|i| starts.get(i).zip(widths.get(i)).is_some_and(|(s, w)| mid < s + w / 2.0))
        .unwrap_or(widths.len().saturating_sub(1))
}

/// The tabs' order with `from` moved to `to`.
fn moved_order(n: usize, from: usize, to: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).filter(|i| *i != from).collect();
    v.insert(to.min(v.len()), from);
    v
}

/// Where index `i` ends up when the item at `from` moves to `to`.
pub fn moved_index(i: usize, from: usize, to: usize) -> usize {
    if i == from {
        to
    } else if from < i && i <= to {
        i - 1
    } else if to <= i && i < from {
        i + 1
    } else {
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_lands_by_the_middles() {
        let w = [100.0, 100.0, 100.0, 100.0];
        // Picked up, not moved: it stays.
        for i in 0..4 {
            assert_eq!(drop_index(&w, 0.0, i, 50.0 + 100.0 * i as f32), i);
        }
        // Tab 0 dragged: its middle past tab 1's middle (150) goes after it.
        assert_eq!(drop_index(&w, 0.0, 0, 140.0), 0);
        assert_eq!(drop_index(&w, 0.0, 0, 160.0), 1);
        assert_eq!(drop_index(&w, 0.0, 0, 999.0), 3);
        // Tab 3 dragged to the front.
        assert_eq!(drop_index(&w, 0.0, 3, 10.0), 0);
        assert_eq!(drop_index(&w, 0.0, 3, 120.0), 1);
        assert_eq!(moved_order(4, 3, 1), [0, 3, 1, 2]);
        assert_eq!(moved_order(4, 0, 3), [1, 2, 3, 0]);
    }

    #[test]
    fn indices_follow_a_move() {
        let v: Vec<usize> = (0..5).collect();
        for (from, to) in [(0, 3), (4, 1), (2, 2)] {
            let order = moved_order(5, from, to);
            for i in v.iter().copied() {
                assert_eq!(order[moved_index(i, from, to)], i, "{from}->{to}, item {i}");
            }
        }
    }

    #[test]
    fn reveal_moves_as_little_as_it_can() {
        assert_eq!(reveal(0.0, 50.0, 90.0, 100.0), 0.0); // in view
        assert_eq!(reveal(0.0, 150.0, 250.0, 100.0), 150.0); // off right
        assert_eq!(reveal(0.0, 150.0, 220.0, 100.0), 120.0);
        assert_eq!(reveal(300.0, 100.0, 180.0, 100.0), 100.0); // off left
        assert_eq!(starts(&[10.0, 20.0, 30.0], 4.0), [0.0, 14.0, 38.0]);
    }
}
