//! Wakes the window for the browser's frames, at full rate whatever the
//! window is doing: shown, covered, minimized, hidden, or the screen locked.
//!
//! Why it's needed (eframe 0.37 / winit 0.30 on macOS): egui's repaint
//! requests go through eframe, which delays them to at least 100 ms apart
//! while the window is minimized or hidden (`INVISIBLE_WINDOW_REPAINT_INTERVAL`,
//! there to stop invisible windows busy-looping on Windows). A browser
//! looking at a minimized window would get ~9 frames a second, and wait up
//! to 100 ms for each key. But winit's own `Window::request_redraw` is
//! never held back on macOS: it queues a `RedrawRequested` that the run
//! loop delivers on its next turn, shown or not, and eframe answers that
//! with a pass that runs `App::logic` (and, while hidden, nothing else —
//! the window isn't painted). So this asks winit directly:
//!
//! - when the browser's copy wants its next frame (`at`), or input or an
//!   ack arrived (`now`), from a small thread that sleeps until then;
//! - while the window is hidden, whenever anything asks the window's own
//!   egui context for a repaint (a terminal's output, a finished search),
//!   since eframe would delay those too: checked every few ms, cheaply.
//!
//! Without a winit window (headless tests) it falls back to egui's request.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

/// How often a hidden window's own repaint requests are looked for.
const HIDDEN_POLL: Duration = Duration::from_millis(4);

#[derive(Default)]
struct State {
    next: Option<Instant>,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    hidden: AtomicBool,
}

pub struct Clock {
    shared: Arc<Shared>,
}

/// Wakes the window now; cheap to clone, for other threads.
#[derive(Clone)]
pub struct Waker(Arc<Shared>);

impl Waker {
    pub fn wake(&self) {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).next = Some(Instant::now());
        self.0.cv.notify_one();
    }

    /// Wake the window at `when` (the earliest asked wins).
    pub fn at(&self, when: Instant) {
        let mut st = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.next.is_none_or(|t| when < t) {
            st.next = Some(when);
            self.0.cv.notify_one();
        }
    }
}

impl Clock {
    /// `redraw` asks winit for a redraw (`None` without a window).
    pub fn start(redraw: Option<Box<dyn Fn() + Send>>, main: egui::Context) -> Clock {
        let shared = Arc::new(Shared { state: Mutex::default(), cv: Condvar::new(), hidden: AtomicBool::new(false) });
        let s = shared.clone();
        let ctx = main.clone();
        let wants = move || ctx.has_requested_repaint_for(&egui::ViewportId::ROOT);
        let wake = move || match &redraw {
            Some(redraw) => redraw(),
            None => main.request_repaint(),
        };
        let _ = std::thread::Builder::new()
            .name("remote-clock".into())
            .spawn(move || run(&s, &wake, &wants));
        Clock { shared }
    }

    /// Wake the window at `when` (the earliest asked wins).
    pub fn at(&self, when: Instant) {
        Waker(self.shared.clone()).at(when);
    }

    pub fn waker(&self) -> Waker {
        Waker(self.shared.clone())
    }

    /// Whether the window is hidden (its own repaints are then forwarded).
    pub fn set_hidden(&self, hidden: bool) {
        if self.shared.hidden.swap(hidden, Ordering::Relaxed) != hidden {
            self.shared.cv.notify_one();
        }
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        self.shared.cv.notify_one();
    }
}

fn run(shared: &Shared, wake: &dyn Fn(), wants: &dyn Fn() -> bool) {
    let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if st.stop {
            return;
        }
        let now = Instant::now();
        let hidden = shared.hidden.load(Ordering::Relaxed);
        let due = st.next.is_some_and(|t| t <= now);
        if due || (hidden && wants()) {
            st.next = None;
            drop(st);
            wake();
            st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            continue;
        }
        let mut wait = st.next.map_or(Duration::from_secs(3600), |t| t - now);
        if hidden {
            wait = wait.min(HIDDEN_POLL);
        }
        st = shared.cv.wait_timeout(st, wait).map_or_else(|e| e.into_inner().0, |r| r.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn wakes_when_asked_and_forwards_a_hidden_windows_repaints() {
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        let main = egui::Context::default();
        let clock = Clock::start(Some(Box::new(move || { w.fetch_add(1, Ordering::SeqCst); })), main.clone());
        let count = || woken.load(Ordering::SeqCst);
        let wait_for = |n: usize| {
            while count() < n {
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(count(), 0, "nothing asked, nothing happens");

        let asked = Instant::now();
        clock.at(asked + Duration::from_millis(40));
        clock.at(asked + Duration::from_millis(500)); // a later ask doesn't delay it
        wait_for(1);
        assert!(asked.elapsed() >= Duration::from_millis(40), "not before it's due");

        clock.waker().wake();
        wait_for(2);

        // Hidden: the window's own pending repaint (a new context has one)
        // wakes it; shown, eframe handles those itself.
        std::thread::sleep(Duration::from_millis(30));
        let before = count();
        assert!(main.has_requested_repaint_for(&egui::ViewportId::ROOT));
        clock.set_hidden(true);
        wait_for(before + 1);
        clock.set_hidden(false);
        std::thread::sleep(Duration::from_millis(20));
        let settled = count();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(count(), settled, "shown again: no more forwarding");
    }
}
