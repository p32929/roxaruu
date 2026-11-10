//! What a browser sends, and how it becomes egui input for the window it's
//! looking at. Positions are CSS pixels, which are egui points: the window
//! is laid out at the browser's size, so nothing needs scaling.
//!
//! The goal is the exact event stream the window gets natively, so that
//! everything downstream (`App::handle_keys`, the editor, the terminal,
//! egui's own widgets) can't tell the difference. The reference, read from
//! egui-winit 0.37 + winit 0.30 on macOS (`on_window_event`,
//! `on_keyboard_input`, winit's `view.rs` / `event.rs`):
//!
//! **Modifiers.** `alt` = ⌥, `ctrl` = ⌃, `shift` = ⇧, `mac_cmd` = ⌘ and
//! `command` = ⌘ only: Ctrl is never the command key on the Mac. Every
//! change emits `ModifiersChanged` (before the next key or click), and every
//! event carries the current set.
//!
//! **A modifier key on its own** (⇧, ⌃, ⌥, ⌘; left or right) emits
//! `Key { key: ShiftLeft… / SuperLeft…, modifiers: the set before }`, then
//! `ModifiersChanged(the set after)`. Release: the same, `pressed: false`.
//!
//! **Any other key** emits `Key { key, physical_key, pressed, modifiers }`
//! where `key` is the logical key (the character typed, with ⇧ and ⌥
//! applied) if egui has a name for it, else the physical key: ⌥T types "†",
//! which isn't a key name, so `key` is T. `repeat` is always false from
//! the backend; egui sets it when a key is pressed again before its
//! release. A dead key (⌥E) has no logical key: it's the physical E.
//!
//! **Text.** On a press only, after the `Key`: `Text(the character)` if it
//! is printable (no control characters, no private-use "function key"
//! characters, so not Enter, Tab, Backspace, arrows) and none of ⌃ / ⌘ is
//! held. ⌥ doesn't stop it: ⌥T sends `Key(T, alt)` + `Text("†")`, and the
//! app drops the text when the key was a shortcut. ⌥E (dead) sends no
//! text; the next key sends the composed "é".
//!
//! **Clipboard.** ⌘C / ⌘X (⌘ without ⌥ or ⇧, by the logical-else-physical
//! key) emit `Copy` / `Cut` *instead of* the key and its text; ⌘V emits
//! `Paste(the clipboard, \r\n → \n)`, also instead, or nothing if empty.
//! ⌃C is an ordinary `Key(C, ctrl)` (the shell's interrupt).
//!
//! **IME.** Only while egui asks for it (a focused `TextEdit`); then
//! composition arrives as `Ime(Preedit / Commit)` with no key events. The
//! editor and terminals don't ask, so on the Mac they get plain keys.
//!
//! **Focus.** Losing focus emits `WindowFocused(false)`; egui then forgets
//! every held key and modifier (no key-ups come for them).
//!
//! **Pointer.** `PointerMoved(points)`, `PointerButton { pos, button,
//! pressed, modifiers }` (left / right / middle / back = Extra1 / forward =
//! Extra2), `PointerGone` on leaving. Double and triple clicks are egui's
//! own timing of presses. Touch ends with `PointerGone` (nothing hovers).
//!
//! **Scrolling.** `MouseWheel { unit, delta, phase, source, modifiers }`,
//! `delta` > 0 moving the content right / down. A trackpad: `Point` units,
//! `Start` when fingers land, `Move` per event, `End` on lifting, source
//! `Trackpad`, then the OS's coasting as source `Momentum`
//! (`Start`/`Move`/`End`). Between `Start` and `End` egui applies deltas
//! as they come, unsmoothed. A mouse wheel: `Line` units, source `Wheel`,
//! smoothed by egui. A pinch is `Zoom(factor)`, not a wheel.
//!
//! The page (`page.html`) sends the browser's facts (`KeyboardEvent.key`,
//! `.code`, the held modifiers, wheel deltas with a gesture phase) and
//! [`Input`] turns them into exactly the above. Browsers differ in two
//! places, handled there: a Mac browser sends no key-up for keys let go
//! while ⌘ is held (the page sends them when ⌘ goes up, as AppKit does), and
//! the browser reports a trackpad pinch as Ctrl+wheel.
//!
//! A phone's on-screen keyboard types into a hidden field on the page,
//! which sends what changed there (Backspaces, then text): autocorrect and
//! input methods rewrite words in place, and the app follows. A swipe in
//! from the left edge asks for the file tree's drawer ([`ClientMsg::Drawer`]).
//!
//! Off a Mac (a PC or phone), Ctrl is that keyboard's command key: it sets
//! both `ctrl` and `command`, so shortcuts reach the same chords, and Ctrl
//! chords in a terminal still go to the shell as on a PC. There, Ctrl+C/X
//! arrive as keys (the keymap's Copy / Cut), not clipboard events, so a
//! shell still gets ^C; ⌘ (the toolbar button, or the Windows key) is ⌘.

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, Vec2};
use serde::Deserialize;

/// Held modifier keys, as the browser reports them.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Mods {
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub meta: bool,
}

impl Mods {
    /// As egui would see these keys on the Mac. From a Mac browser, ⌘ is
    /// the command key and Ctrl stays Ctrl (Ctrl+C interrupts a shell, Ctrl+A
    /// goes to the line start in a text field). From a PC or phone, Ctrl is
    /// its command key, as people there expect.
    pub fn egui(self, mac: bool) -> Modifiers {
        Modifiers {
            alt: self.alt,
            ctrl: self.ctrl,
            shift: self.shift,
            mac_cmd: self.meta,
            command: self.meta || (!mac && self.ctrl),
        }
    }
}

/// `WheelEvent.deltaMode`, named.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WheelUnit {
    #[default]
    Px,
    Line,
    Page,
}

/// Where a scroll is in its gesture (see the module docs).
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WheelPhase {
    Start,
    #[default]
    Move,
    End,
}

/// What drives a scroll: fingers on a trackpad or touch screen, coasting
/// after them, or a mouse wheel.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WheelSource {
    #[default]
    Unknown,
    Wheel,
    Trackpad,
    Momentum,
}

/// One message from the browser (JSON, tagged by `t`).
#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ClientMsg {
    /// The space the window gets in the page, in CSS pixels, and the
    /// device's pixel ratio.
    Size {
        w: f32,
        h: f32,
        dpr: f32,
        /// The browser is on a Mac (⌘ is its command key).
        #[serde(default)]
        mac: bool,
    },
    /// `m`, when sent, is what's held now.
    Move { x: f32, y: f32, #[serde(default)] m: Option<Mods> },
    /// A button pressed; `ts` is when, by the browser's clock
    /// (`Event.timeStamp`, ms).
    Down { x: f32, y: f32, b: u8, #[serde(default)] m: Mods, #[serde(default)] ts: Option<f64> },
    Up { x: f32, y: f32, b: u8, #[serde(default)] m: Mods, #[serde(default)] ts: Option<f64> },
    Leave,
    /// Scroll as the browser reports it: positive `dy` = scrolling down
    /// (the content moves up).
    Wheel {
        dx: f32,
        dy: f32,
        #[serde(default)]
        m: Mods,
        #[serde(default)]
        unit: WheelUnit,
        #[serde(default)]
        phase: WheelPhase,
        #[serde(default)]
        src: WheelSource,
    },
    /// A pinch: the factor to zoom by.
    Zoom { f: f32 },
    /// A key press or release: `k` is `KeyboardEvent.key`, `code` is
    /// `KeyboardEvent.code`, `m` what's held with it (after it, for a
    /// modifier key). What it types is worked out here, so it always lands
    /// in the same frame as the key (a shortcut then swallows it).
    Key {
        k: String,
        #[serde(default)]
        code: String,
        down: bool,
        #[serde(default)]
        m: Mods,
    },
    /// Typed text (phone keyboards and input methods).
    Text { s: String },
    /// This device's clipboard, pasted (⌘V).
    Paste { s: String },
    Copy,
    Cut,
    /// The page gained or lost the keyboard (switched tab, other window).
    Focus { on: bool },
    /// The browser tab was hidden or shown: frames can pause.
    Visible { on: bool },
    /// A finger swiped in from the left edge: the file tree's drawer comes
    /// out (in a compact view; handled by `serve`, not as egui input).
    Drawer,
}

impl ClientMsg {
    pub fn parse(json: &str) -> Option<ClientMsg> {
        serde_json::from_str(json).ok()
    }
}

/// The egui key for a browser key: the logical one from `key` if egui has
/// a name for it, else the physical one from `code` (⌥T's "†", a dead
/// key), and the physical one.
pub fn key_of(key: &str, code: &str) -> Option<(Key, Option<Key>)> {
    let physical = Key::from_name(code);
    let logical = Key::from_name(key).or(physical)?;
    Some((logical, physical))
}

/// What a key press types: `KeyboardEvent.key` is the character, or a
/// name ("Enter", "F1", "Dead") — names are ASCII words — and only
/// printable characters count, as natively.
fn typed(key: &str) -> Option<&str> {
    let name = key.chars().count() > 1 && key.chars().all(|c| c.is_ascii_alphanumeric());
    let printable = |c: char| {
        !c.is_ascii_control()
            && !('\u{e000}'..='\u{f8ff}').contains(&c)
            && !('\u{f0000}'..='\u{10fffd}').contains(&c)
    };
    (!key.is_empty() && !name && key.chars().all(printable)).then_some(key)
}

fn button(b: u8) -> PointerButton {
    match b {
        1 => PointerButton::Middle,
        2 => PointerButton::Secondary,
        3 => PointerButton::Extra1,
        4 => PointerButton::Extra2,
        _ => PointerButton::Primary,
    }
}

/// One browser's keyboard and pointer, as egui-winit on the Mac would
/// report them: remembers what's held, so modifier changes are announced
/// once, before the event they come with.
///
/// It also keeps the clock for the browser's copy of the window. egui
/// tells a double-click from two clicks by the time between them, and
/// natively that's when the person clicked. Over Wi-Fi, clicks can arrive
/// bunched (or spread out), so each pass is timed by when its click
/// happened in the browser, mapped onto the window's clock; and a pass
/// takes at most one press ([`Input::splits_before`]).
#[derive(Default)]
pub struct Input {
    /// The browser is on a Mac (⌘ is its command key).
    pub mac: bool,
    held: Mods,
    /// The window's clock now, and the browser's clock's offset to it (the
    /// smallest seen: the quickest delivery), in seconds.
    now: f64,
    offset: Option<f64>,
    /// When this pass's press happened, on the window's clock.
    pressed_at: Option<f64>,
    /// The last pass's time (passes never go back in time).
    last: f64,
}

impl Input {
    /// Start a pass's worth of messages at `now` (seconds, the window's clock).
    pub fn begin(&mut self, now: f64) {
        self.now = now;
        self.pressed_at = None;
    }

    /// Should `msg` wait for the next pass? Yes, after a button press, for
    /// a second press (two presses in one pass would count as a double-click
    /// however far apart they were) and for typing: the app acts on keys
    /// before it draws, where clicks land, so ⇧-click then ⌘X arriving
    /// together would cut before the click had selected anything.
    pub fn splits_before(&self, msg: &ClientMsg) -> bool {
        self.pressed_at.is_some()
            && matches!(
                msg,
                ClientMsg::Down { .. }
                    | ClientMsg::Key { down: true, .. }
                    | ClientMsg::Text { .. }
                    | ClientMsg::Paste { .. }
                    | ClientMsg::Copy
                    | ClientMsg::Cut
            )
    }

    /// The time for this pass: when its press happened, else now.
    pub fn time(&mut self) -> f64 {
        self.last = self.last.max(self.pressed_at.unwrap_or(self.now));
        self.last
    }

    /// A browser timestamp (ms) on the window's clock.
    fn when(&mut self, ts: Option<f64>) -> f64 {
        let Some(ts) = ts.map(|ms| ms / 1000.0).filter(|t| t.is_finite()) else {
            return self.now;
        };
        let offset = self.offset.map_or(self.now - ts, |o| o.min(self.now - ts));
        self.offset = Some(offset);
        ts + offset
    }

    fn hold(&mut self, m: Mods, out: &mut Vec<Event>) {
        if m != self.held {
            self.held = m;
            out.push(Event::ModifiersChanged(m.egui(self.mac)));
        }
    }

    fn mods(&self) -> Modifiers {
        self.held.egui(self.mac)
    }

    /// The text a press of `key` types: none on release or with ⌃ / ⌘.
    fn text(&self, key: &str, down: bool) -> Option<Event> {
        let m = self.mods();
        let chord = m.ctrl || m.command || m.mac_cmd;
        typed(key).filter(|_| down && !chord).map(|t| Event::Text(t.to_string()))
    }

    /// The egui events for `msg` (a size change returns none: it's handled
    /// as the screen size).
    pub fn events(&mut self, msg: &ClientMsg) -> Vec<Event> {
        let mut out = Vec::new();
        match msg {
            ClientMsg::Move { x, y, m } => {
                if let Some(m) = m {
                    self.hold(*m, &mut out);
                }
                out.push(Event::PointerMoved(Pos2::new(*x, *y)));
            }
            ClientMsg::Down { x, y, b, m, ts } | ClientMsg::Up { x, y, b, m, ts } => {
                let at = self.when(*ts);
                if matches!(msg, ClientMsg::Down { .. }) {
                    self.pressed_at.get_or_insert(at);
                }
                self.hold(*m, &mut out);
                let pos = Pos2::new(*x, *y);
                out.push(Event::PointerMoved(pos));
                out.push(Event::PointerButton {
                    pos,
                    button: button(*b),
                    pressed: matches!(msg, ClientMsg::Down { .. }),
                    modifiers: self.mods(),
                });
            }
            ClientMsg::Leave => out.push(Event::PointerGone),
            ClientMsg::Wheel { dx, dy, m, unit, phase, src } => {
                self.hold(*m, &mut out);
                let unit = match unit {
                    WheelUnit::Px => egui::MouseWheelUnit::Point,
                    WheelUnit::Line => egui::MouseWheelUnit::Line,
                    WheelUnit::Page => egui::MouseWheelUnit::Page,
                };
                let source = match src {
                    WheelSource::Unknown => egui::MouseWheelSource::Unknown,
                    WheelSource::Wheel => egui::MouseWheelSource::Wheel,
                    WheelSource::Trackpad => egui::MouseWheelSource::Trackpad,
                    WheelSource::Momentum => egui::MouseWheelSource::Momentum,
                };
                let modifiers = self.mods();
                let wheel = |phase, delta| Event::MouseWheel { unit, delta, phase, source, modifiers };
                let delta = Vec2::new(-*dx, -*dy);
                // egui ignores a Start's delta: it only opens the gesture.
                if *phase == WheelPhase::Start {
                    out.push(wheel(egui::TouchPhase::Start, Vec2::ZERO));
                }
                if *phase == WheelPhase::End {
                    out.push(wheel(egui::TouchPhase::End, Vec2::ZERO));
                } else if delta != Vec2::ZERO || *phase == WheelPhase::Move {
                    out.push(wheel(egui::TouchPhase::Move, delta));
                }
            }
            ClientMsg::Zoom { f } if f.is_finite() && *f > 0.0 => out.push(Event::Zoom(*f)),
            ClientMsg::Key { k, code, down, m } => {
                let Some((key, physical_key)) = key_of(k, code) else {
                    // No key egui names (a symbol on a key with no known
                    // code): natively it still types.
                    self.hold(*m, &mut out);
                    out.extend(self.text(k, *down));
                    return out;
                };
                if crate::keymap::is_modifier(physical_key.unwrap_or(key)) {
                    // The key first (with what was held before), then the change.
                    out.push(Event::Key { key, physical_key, pressed: *down, repeat: false, modifiers: self.mods() });
                    self.hold(*m, &mut out);
                    return out;
                }
                self.hold(*m, &mut out);
                let modifiers = self.mods();
                let clip = modifiers.mac_cmd && !modifiers.alt && !modifiers.shift;
                if *down && (key == Key::Copy || (clip && key == Key::C)) {
                    out.push(Event::Copy);
                } else if *down && (key == Key::Cut || (clip && key == Key::X)) {
                    out.push(Event::Cut);
                } else {
                    out.push(Event::Key { key, physical_key, pressed: *down, repeat: false, modifiers });
                    out.extend(self.text(k, *down));
                }
            }
            ClientMsg::Text { s } if !s.is_empty() => out.push(Event::Text(s.clone())),
            ClientMsg::Paste { s } if !s.is_empty() => out.push(Event::Paste(s.replace("\r\n", "\n"))),
            ClientMsg::Copy => out.push(Event::Copy),
            ClientMsg::Cut => out.push(Event::Cut),
            ClientMsg::Focus { on } => {
                // egui forgets held keys and modifiers on its own; so do we.
                if !on {
                    self.held = Mods::default();
                }
                out.push(Event::WindowFocused(*on));
            }
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{COMMANDS, Combo, Keymap};

    const NONE: Mods = Mods { alt: false, ctrl: false, shift: false, meta: false };
    const ALT: Mods = Mods { alt: true, ..NONE };
    const SHIFT: Mods = Mods { shift: true, ..NONE };
    const META: Mods = Mods { meta: true, ..NONE };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };

    fn key(k: &str, code: &str, down: bool, m: Mods) -> ClientMsg {
        ClientMsg::Key { k: k.into(), code: code.into(), down, m }
    }

    /// What a Mac browser sends for a press (modifiers already down, as
    /// separate presses) — the events after the modifier announcements.
    fn press(input: &mut Input, k: &str, code: &str, m: Mods) -> Vec<Event> {
        let ev = input.events(&key(k, code, true, m));
        ev.into_iter().filter(|e| !matches!(e, Event::ModifiersChanged(_))).collect()
    }

    fn mac() -> Input {
        Input { mac: true, ..Input::default() }
    }

    fn egui_mods(alt: bool, ctrl: bool, shift: bool, cmd: bool) -> Modifiers {
        Modifiers { alt, ctrl, shift, mac_cmd: cmd, command: cmd }
    }

    #[test]
    fn parses_what_the_page_sends() {
        assert_eq!(
            ClientMsg::parse(r#"{"t":"size","w":800,"h":600,"dpr":2,"mac":true}"#),
            Some(ClientMsg::Size { w: 800.0, h: 600.0, dpr: 2.0, mac: true })
        );
        // An older page's "text" field is ignored: the text is worked out here.
        assert_eq!(
            ClientMsg::parse(r#"{"t":"key","k":"w","code":"KeyW","down":true,"m":{"meta":true},"text":"w"}"#),
            Some(key("w", "KeyW", true, META))
        );
        assert_eq!(
            ClientMsg::parse(r#"{"t":"wheel","dx":0,"dy":3,"phase":"start","src":"trackpad"}"#),
            Some(ClientMsg::Wheel {
                dx: 0.0,
                dy: 3.0,
                m: NONE,
                unit: WheelUnit::Px,
                phase: WheelPhase::Start,
                src: WheelSource::Trackpad
            })
        );
        assert_eq!(ClientMsg::parse(r#"{"t":"focus","on":false}"#), Some(ClientMsg::Focus { on: false }));
        assert_eq!(ClientMsg::parse(r#"{"t":"copy"}"#), Some(ClientMsg::Copy));
        assert_eq!(ClientMsg::parse(r#"{"t":"drawer"}"#), Some(ClientMsg::Drawer));
        assert!(mac().events(&ClientMsg::Drawer).is_empty(), "not egui input");
        assert_eq!(ClientMsg::parse(r#"{"t":"nope"}"#), None);
        assert_eq!(ClientMsg::parse("not json"), None);
    }

    #[test]
    fn browser_keys_become_egui_keys() {
        assert_eq!(key_of("ArrowDown", "ArrowDown"), Some((Key::ArrowDown, Some(Key::ArrowDown))));
        assert_eq!(key_of("w", "KeyW"), Some((Key::W, Some(Key::W))));
        // ⌥W on a Mac types "∑": the physical key still says W.
        assert_eq!(key_of("∑", "KeyW"), Some((Key::W, Some(Key::W))));
        assert_eq!(key_of("Dead", "KeyE"), Some((Key::E, Some(Key::E))), "a dead key is its physical key");
        assert_eq!(key_of("[", "BracketLeft"), Some((Key::OpenBracket, Some(Key::OpenBracket))));
        assert_eq!(key_of("=", "Equal"), Some((Key::Equals, Some(Key::Equals))));
        assert_eq!(key_of(" ", "Space"), Some((Key::Space, Some(Key::Space))));
        assert_eq!(key_of("Meta", "MetaLeft"), Some((Key::SuperLeft, Some(Key::SuperLeft))));
        assert_eq!(key_of("Unidentified", ""), None, "phone keyboards' placeholder isn't a key");
    }

    #[test]
    fn only_printable_characters_type() {
        assert_eq!(typed("a"), Some("a"));
        assert_eq!(typed(" "), Some(" "));
        assert_eq!(typed("†"), Some("†"));
        assert_eq!(typed("👍🏽"), Some("👍🏽"));
        assert_eq!(typed("´t"), Some("´t"), "a dead key that didn't combine types both");
        for name in ["Enter", "Tab", "Backspace", "Escape", "F1", "ArrowLeft", "Dead", "Shift", ""] {
            assert_eq!(typed(name), None, "{name}");
        }
        assert_eq!(typed("\u{f728}"), None, "AppKit's private-use function key characters");
    }

    #[test]
    fn cmd_and_ctrl_both_reach_shortcuts() {
        let mac = META.egui(true);
        let pc = CTRL.egui(false);
        let combo = |m| Combo::from_press(Key::W, Some(Key::W), m);
        assert_eq!(combo(mac), combo(pc), "⌘W from a Mac, Ctrl+W from a PC: the same chord");
        // From a Mac, Ctrl isn't the command key (as on the Mac itself).
        let ctrl_on_mac = CTRL.egui(true);
        assert!(ctrl_on_mac.ctrl && !ctrl_on_mac.command && !ctrl_on_mac.mac_cmd);
        assert!(pc.command, "a PC's Ctrl is its command key");
    }

    /// ⌥T on a Mac: `Key(T, ⌥)` then `Text("†")`, both from one message, so
    /// one frame: a shortcut swallows the text. No text on release.
    #[test]
    fn option_letter_is_key_then_text() {
        let mut i = mac();
        let ev = i.events(&key("Alt", "AltLeft", true, ALT));
        assert_eq!(
            ev,
            vec![
                Event::Key { key: Key::AltLeft, physical_key: Some(Key::AltLeft), pressed: true, repeat: false, modifiers: Modifiers::NONE },
                Event::ModifiersChanged(egui_mods(true, false, false, false)),
            ],
            "a modifier on its own: its key (with what was held before), then the change"
        );
        let ev = i.events(&key("†", "KeyT", true, ALT));
        assert_eq!(
            ev,
            vec![
                Event::Key { key: Key::T, physical_key: Some(Key::T), pressed: true, repeat: false, modifiers: egui_mods(true, false, false, false) },
                Event::Text("†".into()),
            ]
        );
        assert!(!i.events(&key("†", "KeyT", false, ALT)).iter().any(|e| matches!(e, Event::Text(_))));
        let ev = i.events(&key("Alt", "AltLeft", false, NONE));
        assert!(matches!(ev[0], Event::Key { key: Key::AltLeft, pressed: false, .. }));
        assert_eq!(ev[1], Event::ModifiersChanged(Modifiers::NONE));
    }

    /// ⌥E then E on a US Mac: the dead key types nothing, the next key the
    /// accented letter.
    #[test]
    fn dead_keys_compose() {
        let mut i = mac();
        let ev = press(&mut i, "Dead", "KeyE", ALT);
        assert_eq!(ev.len(), 1);
        assert!(matches!(ev[0], Event::Key { key: Key::E, .. }));
        i.events(&key("Dead", "KeyE", false, ALT));
        let ev = press(&mut i, "é", "KeyE", NONE);
        assert_eq!(ev[1], Event::Text("é".into()));
        // A character on a key with no code egui knows still types.
        assert_eq!(press(&mut i, "ろ", "IntlRo", NONE), vec![Event::Text("ろ".into())]);
        assert!(press(&mut i, "ろ", "IntlRo", META).is_empty());
    }

    /// Space, Enter, Tab, Backspace, Delete and arrows: keys; only Space
    /// types, and not with ⌘ or ⌃.
    #[test]
    fn special_keys_with_and_without_modifiers() {
        let mut i = mac();
        for (k, m) in [(" ", NONE), (" ", SHIFT), (" ", ALT)] {
            let ev = press(&mut i, k, "Space", m);
            assert_eq!(ev.last(), Some(&Event::Text(" ".into())), "{m:?}");
        }
        for m in [META, CTRL] {
            let ev = press(&mut i, " ", "Space", m);
            assert_eq!(ev.len(), 1, "no text with {m:?}");
        }
        for (k, want) in [
            ("Enter", Key::Enter),
            ("Tab", Key::Tab),
            ("Backspace", Key::Backspace),
            ("Delete", Key::Delete),
            ("ArrowLeft", Key::ArrowLeft),
            ("Home", Key::Home),
            ("PageDown", Key::PageDown),
        ] {
            for m in [NONE, SHIFT, ALT, META, CTRL, Mods { shift: true, meta: true, ..NONE }] {
                let ev = press(&mut i, k, k, m);
                assert_eq!(ev.len(), 1, "{k} {m:?} types nothing");
                let Event::Key { key, modifiers, .. } = ev[0] else { panic!("{k}") };
                assert_eq!(key, want);
                assert_eq!(modifiers, m.egui(true));
            }
        }
    }

    /// The browser repeats a held key as more presses; egui marks them
    /// repeats itself, as natively (the backend never does).
    #[test]
    fn repeats_are_plain_presses() {
        let mut i = mac();
        for _ in 0..3 {
            let ev = press(&mut i, "r", "KeyR", NONE);
            assert!(matches!(ev[0], Event::Key { key: Key::R, pressed: true, repeat: false, .. }));
            assert_eq!(ev[1], Event::Text("r".into()));
        }
    }

    /// ⌘C / ⌘X are clipboard events instead of keys; ⌃C (the shell's
    /// interrupt) and ⌘⇧C stay keys. Off a Mac, Ctrl+C is a key too.
    #[test]
    fn clipboard_chords() {
        let mut i = mac();
        assert_eq!(press(&mut i, "c", "KeyC", META), vec![Event::Copy]);
        assert_eq!(press(&mut i, "x", "KeyX", META), vec![Event::Cut]);
        assert_eq!(press(&mut i, "c", "KeyC", Mods { ctrl: true, meta: true, ..NONE }), vec![Event::Copy]);
        assert!(matches!(press(&mut i, "c", "KeyC", CTRL)[..], [Event::Key { key: Key::C, .. }]));
        assert!(matches!(press(&mut i, "c", "KeyC", Mods { shift: true, meta: true, ..NONE })[..], [Event::Key { .. }]));
        assert!(matches!(press(&mut i, "ç", "KeyC", Mods { alt: true, meta: true, ..NONE })[..], [Event::Key { .. }]));
        // The release of ⌘C is an ordinary key-up.
        assert!(matches!(i.events(&key("c", "KeyC", false, META))[..], [Event::ModifiersChanged(_), Event::Key { pressed: false, .. }]));
        let mut pc = Input::default();
        assert!(matches!(press(&mut pc, "c", "KeyC", CTRL)[..], [Event::Key { key: Key::C, .. }]));
        assert_eq!(press(&mut pc, "c", "KeyC", META), vec![Event::Copy]);
        // Pasted text comes from the browser, with Windows line ends fixed.
        assert_eq!(i.events(&ClientMsg::Paste { s: "a\r\nb".into() }), vec![Event::Paste("a\nb".into())]);
        assert!(i.events(&ClientMsg::Paste { s: String::new() }).is_empty());
    }

    /// Modifier changes are announced once, before the event they come with.
    #[test]
    fn modifier_changes_come_first_and_once() {
        let mut i = mac();
        let ev = i.events(&ClientMsg::Down { x: 1.0, y: 2.0, b: 0, m: SHIFT, ts: None });
        assert_eq!(ev[0], Event::ModifiersChanged(SHIFT.egui(true)));
        assert!(matches!(ev[2], Event::PointerButton { modifiers, .. } if modifiers.shift));
        let ev = i.events(&ClientMsg::Up { x: 1.0, y: 2.0, b: 0, m: SHIFT, ts: None });
        assert!(!ev.iter().any(|e| matches!(e, Event::ModifiersChanged(_))), "unchanged: not announced");
        let ev = i.events(&ClientMsg::Move { x: 3.0, y: 3.0, m: None });
        assert_eq!(ev, vec![Event::PointerMoved(Pos2::new(3.0, 3.0))], "a bare move keeps them");
        let ev = i.events(&ClientMsg::Move { x: 3.0, y: 3.0, m: Some(NONE) });
        assert_eq!(ev[0], Event::ModifiersChanged(Modifiers::NONE));
    }

    /// Switching away with keys held: egui is told, and forgets them.
    #[test]
    fn losing_focus_lets_go() {
        let mut i = mac();
        i.events(&key("Shift", "ShiftLeft", true, SHIFT));
        assert_eq!(i.events(&ClientMsg::Focus { on: false }), vec![Event::WindowFocused(false)]);
        let ev = press(&mut i, "k", "KeyK", NONE);
        assert_eq!(ev[1], Event::Text("k".into()));
        assert_eq!(i.held, NONE);
    }

    /// Clicks are timed by when they happened in the browser: two clicks
    /// half a second apart that arrive together are two passes, half a
    /// second apart; a quick double-click stays quick.
    #[test]
    fn clicks_keep_their_own_timing() {
        let mut i = mac();
        let down = |ts| ClientMsg::Down { x: 1.0, y: 1.0, b: 0, m: NONE, ts: Some(ts) };
        let up = |ts| ClientMsg::Up { x: 1.0, y: 1.0, b: 0, m: NONE, ts: Some(ts) };
        // The link's quickest delivery sets the clocks' offset.
        i.begin(10.0);
        i.events(&ClientMsg::Move { x: 0.0, y: 0.0, m: None });
        assert_eq!(i.time(), 10.0);
        i.begin(10.6);
        for m in [down(600.0), up(650.0)] {
            assert!(!i.splits_before(&m));
            i.events(&m);
        }
        assert!(i.splits_before(&down(1150.0)), "a second press waits for the next pass");
        assert!((i.time() - 10.6).abs() < 1e-9);
        // Delayed by the network: arrives later, keeps its own time.
        i.begin(11.9);
        i.events(&down(1150.0));
        // (The release that came with the first press showed a quicker
        // delivery, so the clocks' offset is now 50 ms smaller.)
        assert!((i.time() - 11.10).abs() < 1e-9, "half a second after the first click");
        // Time never goes back, and a pass without a press runs now.
        i.begin(12.0);
        assert_eq!(i.time(), 12.0);
        i.begin(12.1);
        i.events(&down(1100.0));
        assert_eq!(i.time(), 12.0);
    }

    /// A key after a click in the same batch waits for the click's pass to
    /// be drawn; keys on their own and a release don't.
    #[test]
    fn keys_after_a_click_wait_for_it() {
        let mut i = mac();
        i.begin(1.0);
        assert!(!i.splits_before(&key("x", "KeyX", true, META)), "no click yet");
        i.events(&ClientMsg::Down { x: 1.0, y: 1.0, b: 0, m: SHIFT, ts: None });
        assert!(!i.splits_before(&ClientMsg::Up { x: 1.0, y: 1.0, b: 0, m: SHIFT, ts: None }));
        assert!(!i.splits_before(&key("Shift", "ShiftLeft", false, NONE)), "a release");
        for m in [key("x", "KeyX", true, META), ClientMsg::Text { s: "a".into() }, ClientMsg::Paste { s: "a".into() }, ClientMsg::Cut] {
            assert!(i.splits_before(&m), "{m:?}");
        }
    }

    #[test]
    fn pointer_buttons() {
        let mut i = mac();
        for (b, want) in [
            (0, PointerButton::Primary),
            (1, PointerButton::Middle),
            (2, PointerButton::Secondary),
            (3, PointerButton::Extra1),
            (4, PointerButton::Extra2),
        ] {
            let ev = i.events(&ClientMsg::Down { x: 10.0, y: 20.0, b, m: NONE, ts: None });
            assert_eq!(ev[0], Event::PointerMoved(Pos2::new(10.0, 20.0)));
            assert!(matches!(ev[1], Event::PointerButton { button, pressed: true, .. } if button == want));
        }
        // ⌘-click: the click carries ⌘.
        let ev = i.events(&ClientMsg::Down { x: 1.0, y: 1.0, b: 0, m: META, ts: None });
        assert!(matches!(ev[2], Event::PointerButton { modifiers, .. } if modifiers.command && modifiers.mac_cmd));
        assert_eq!(i.events(&ClientMsg::Leave), vec![Event::PointerGone]);
    }

    /// A trackpad gesture: Start (no delta), exact moves, End; content
    /// moves opposite to the browser's scroll direction.
    #[test]
    fn trackpad_and_wheel_scrolling() {
        let mut i = mac();
        let wheel = |phase, src, dy| ClientMsg::Wheel { dx: 0.0, dy, m: NONE, unit: WheelUnit::Px, phase, src };
        let ev = i.events(&wheel(WheelPhase::Start, WheelSource::Trackpad, 3.5));
        assert!(matches!(ev[0], Event::MouseWheel { phase: egui::TouchPhase::Start, delta, source: egui::MouseWheelSource::Trackpad, .. } if delta == Vec2::ZERO));
        assert!(matches!(ev[1], Event::MouseWheel { phase: egui::TouchPhase::Move, delta, unit: egui::MouseWheelUnit::Point, .. } if delta.y == -3.5));
        let ev = i.events(&wheel(WheelPhase::End, WheelSource::Trackpad, 0.0));
        assert!(matches!(ev[..], [Event::MouseWheel { phase: egui::TouchPhase::End, .. }]));
        let ev = i.events(&wheel(WheelPhase::Move, WheelSource::Momentum, 2.0));
        assert!(matches!(ev[..], [Event::MouseWheel { source: egui::MouseWheelSource::Momentum, .. }]));
        let ev = i.events(&ClientMsg::Wheel { dx: 0.0, dy: 3.0, m: SHIFT, unit: WheelUnit::Line, phase: WheelPhase::Move, src: WheelSource::Wheel });
        assert_eq!(ev[0], Event::ModifiersChanged(SHIFT.egui(true)));
        assert!(matches!(ev[1], Event::MouseWheel { unit: egui::MouseWheelUnit::Line, modifiers, .. } if modifiers.shift));
        assert_eq!(i.events(&ClientMsg::Zoom { f: 1.1 }), vec![Event::Zoom(1.1)]);
        assert!(i.events(&ClientMsg::Zoom { f: f32::NAN }).is_empty());
        assert!(i.events(&ClientMsg::Size { w: 1.0, h: 1.0, dpr: 1.0, mac: true }).is_empty());
        assert!(i.events(&ClientMsg::Text { s: String::new() }).is_empty());
    }

    /// The DOM `code` for a key egui names (a US layout).
    fn code_of(key: Key) -> String {
        let name = key.name();
        match name.len() {
            1 if name.chars().all(|c| c.is_ascii_alphabetic()) => format!("Key{name}"),
            1 if name.chars().all(|c| c.is_ascii_digit()) => format!("Digit{name}"),
            _ => match key {
                Key::Comma => "Comma".into(),
                Key::Period => "Period".into(),
                Key::Minus => "Minus".into(),
                Key::Equals => "Equal".into(),
                Key::OpenBracket => "BracketLeft".into(),
                Key::CloseBracket => "BracketRight".into(),
                Key::Backslash => "Backslash".into(),
                Key::Slash => "Slash".into(),
                _ => name.to_string(),
            },
        }
    }

    /// What a US keyboard types for a key (unshifted, shifted).
    fn chars_of(key: Key) -> (String, String) {
        let n = key.name();
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        match key {
            Key::Comma => pair(",", "<"),
            Key::Period => pair(".", ">"),
            Key::Minus => pair("-", "_"),
            Key::Equals => pair("=", "+"),
            Key::OpenBracket => pair("[", "{"),
            Key::CloseBracket => pair("]", "}"),
            Key::Backslash => pair("\\", "|"),
            Key::Slash => pair("/", "?"),
            Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6
            | Key::Num7 | Key::Num8 | Key::Num9 => {
                let shifted = ")!@#$%^&*(";
                let d = n.parse::<usize>().unwrap_or(0);
                (n.to_string(), shifted[d..=d].to_string())
            }
            _ if n.len() == 1 => (n.to_ascii_lowercase(), n.to_string()),
            _ => pair(n, n), // named keys report their name
        }
    }

    /// Every default shortcut, pressed in a Mac browser and in a PC
    /// browser the way a person would (modifier keys first, then the key
    /// as that browser reports it: ⌥ turns letters into odd characters, ⇧
    /// shifts them), reaches the same keymap command as on the Mac — or,
    /// for ⌘C / ⌘X, the clipboard event the Mac sends for them.
    #[test]
    fn every_default_shortcut_from_any_browser_matches_the_mac() {
        let (km, _) = Keymap::new(&[]);
        for c in COMMANDS {
            for combo in km.combos_for(c.command) {
                let (key, native) = combo.press();
                // Natively on the Mac (egui-winit): ⌘C/⌘X become clipboard
                // events, everything else a key that the keymap matches.
                let mac_copy = native.command_only() && matches!(key, Key::C | Key::X);
                for browser_mac in [true, false] {
                    let mut i = Input { mac: browser_mac, ..Input::default() };
                    let m = Mods {
                        alt: native.alt,
                        shift: native.shift,
                        // ⌘ on a Mac, Ctrl on a PC.
                        meta: native.mac_cmd && browser_mac,
                        ctrl: native.mac_cmd && !browser_mac,
                    };
                    let (plain, shifted) = chars_of(key);
                    let k = if m.alt && key.name().len() == 1 {
                        "∂".to_string() // what ⌥ makes of it on a Mac
                    } else if m.shift {
                        shifted
                    } else {
                        plain
                    };
                    let code = code_of(key);
                    let ev = press(&mut i, &k, &code, m);
                    let what = format!("{} ({}, {} browser)", c.id, combo.to_config(), if browser_mac { "Mac" } else { "PC" });
                    if mac_copy && browser_mac {
                        assert_eq!(ev, vec![if key == Key::C { Event::Copy } else { Event::Cut }], "{what}");
                        continue;
                    }
                    let Some(Event::Key { key: k2, physical_key, modifiers, .. }) = ev.first() else {
                        panic!("{what}: {ev:?}");
                    };
                    assert_eq!(km.command_for(*k2, *physical_key, *modifiers), Some(c.command), "{what}");
                    assert_eq!(Combo::from_press(*k2, *physical_key, *modifiers), combo, "{what}");
                    // A chord's typed character never reaches anything: with
                    // ⌘/⌃ there's none; with ⌥ alone it's right after the key,
                    // where the app drops it.
                    if let Some(t) = ev.get(1) {
                        assert!(m.alt && !m.meta && !m.ctrl && matches!(t, Event::Text(_)), "{what}: {ev:?}");
                    }
                }
            }
        }
    }
}
