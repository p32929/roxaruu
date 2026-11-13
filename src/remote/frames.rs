//! Frames leave on their own thread: each is compressed there (`meshes.rs`
//! says what's in one), so the window never waits for it. One at a time:
//! while it's busy the window holds its newest pass back, and the next
//! frame shows everything that changed meanwhile.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// What sending has cost so far (the log's "remote timing", tests).
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub frames: u64,
    pub bytes: u64,
    pub encode_secs: f64,
}

/// The sending thread.
pub struct Encoder {
    slot: SyncSender<Vec<u8>>,
    busy: Arc<AtomicBool>,
    pub stats: Arc<Mutex<Stats>>,
}

impl Encoder {
    /// `send` gets each frame's message; `done` is called when the thread
    /// is free again.
    pub fn start(send: impl Fn(Vec<u8>) + Send + 'static, done: impl Fn() + Send + 'static) -> Encoder {
        let (slot, frames) = sync_channel::<Vec<u8>>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(Stats::default()));
        let (b, st) = (busy.clone(), stats.clone());
        let _ = std::thread::Builder::new()
            .name("remote-encoder".into())
            .spawn(move || run(&frames, &b, &st, &send, &done));
        Encoder { slot, busy, stats }
    }

    /// Whether a new frame would wait (one is queued, or being sent).
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Hand over a frame (`meshes::frame`), to compress and send; false if
    /// the thread is gone or full.
    pub fn submit(&self, frame: Vec<u8>) -> bool {
        self.busy.store(true, Ordering::Release);
        match self.slot.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }
}

fn run(frames: &Receiver<Vec<u8>>, busy: &AtomicBool, stats: &Mutex<Stats>, send: &dyn Fn(Vec<u8>), done: &dyn Fn()) {
    // Until the `Encoder` (the sending side) is dropped.
    for frame in frames {
        let t = Instant::now();
        let msg = super::meshes::compress(&frame);
        let mut s = stats.lock().unwrap_or_else(|e| e.into_inner());
        s.frames += 1;
        s.bytes += msg.len() as u64;
        s.encode_secs += t.elapsed().as_secs_f64();
        drop(s);
        send(msg);
        busy.store(false, Ordering::Release);
        done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_go_out_compressed_one_at_a_time() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let enc = Encoder::start(move |m| drop(tx.send(m)), move || { let _ = done_tx.send(()); });
        assert!(enc.submit(vec![7; 10_000]));
        let msg = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(msg[0], super::super::meshes::KIND);
        assert!(msg.len() < 200, "compressed: {} bytes", msg.len());
        done_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(!enc.busy(), "free again once sent");
        assert_eq!(enc.stats.lock().unwrap().frames, 1);
    }
}
