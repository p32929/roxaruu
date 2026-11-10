//! A window's private link for remote access: a WebSocket server on
//! localhost that only the server (the hub) talks to, proving itself with
//! this window's token. What browsers send queues up for the window's next
//! remote frame; frames and clipboard text go back out.
//!
//! Each link has a reading thread and a writing thread, both blocked until
//! there's something to do: input reaches the window, and frames leave it,
//! the moment they exist (no polling in between).
//!
//! Flow control: the page acks every frame once drawn. A browser with
//! `IN_FLIGHT` frames unacked gets no more until it catches up, so a slow
//! link never queues stale frames: the next one sent shows everything
//! that changed meanwhile.

use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use tungstenite::Message;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::Role;

use super::input::ClientMsg;

/// The header carrying the window's token from the hub.
pub const TOKEN_HEADER: &str = "x-rox-token";
/// The page's message once it has drawn a frame.
pub const ACK: &str = r#"{"t":"ack"}"#;
/// Frames a browser may have unacked: two keeps the link busy while the
/// page draws one, without letting a queue build.
const IN_FLIGHT: u32 = 2;
/// A browser that acks nothing for this long is sent frames anyway (one
/// that doesn't ack at all still gets a frame now and then).
pub const ACK_PATIENCE: Duration = Duration::from_secs(2);

/// Something for the browsers looking at this window.
#[derive(Clone)]
pub enum Out {
    Frame(Arc<Vec<u8>>),
    Text(String),
}

struct Client {
    tx: mpsc::Sender<Out>,
    /// Frames sent and not yet acked.
    unacked: Arc<AtomicU32>,
    /// When it last acked (or joined).
    heard: Arc<Mutex<Instant>>,
    gone: Arc<AtomicBool>,
}

impl Client {
    fn caught_up(&self) -> bool {
        self.unacked.load(Ordering::Acquire) < IN_FLIGHT
            || self.heard.lock().unwrap_or_else(|e| e.into_inner()).elapsed() > ACK_PATIENCE
    }
}

#[derive(Default)]
struct Shared {
    inbox: Mutex<Vec<ClientMsg>>,
    clients: Mutex<Vec<Client>>,
    /// Every link's socket, to end them when the window closes its link.
    sockets: Mutex<Vec<(Option<SocketAddr>, TcpStream)>>,
    /// A browser just joined: it needs a frame now.
    joined: AtomicBool,
    /// The window held a frame back for a browser to catch up: wake it on
    /// the next ack.
    waiting: AtomicBool,
}

impl Shared {
    fn broadcast(&self, out: Out) {
        let frame = matches!(out, Out::Frame(_));
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| {
            if frame {
                c.unacked.fetch_add(1, Ordering::AcqRel);
            }
            !c.gone.load(Ordering::Acquire) && c.tx.send(out.clone()).is_ok()
        });
    }
}

/// A handle for sending from other threads (the encoder).
#[derive(Clone)]
pub struct Sender(Arc<Shared>);

impl Sender {
    pub fn broadcast(&self, out: Out) {
        self.0.broadcast(out);
    }
}

pub struct Endpoint {
    pub port: u16,
    pub token: String,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
}

impl Endpoint {
    /// Listen on a free localhost port. `wake` is called when a browser sends
    /// something (so the window draws its next remote frame).
    pub fn start(token: String, wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Endpoint> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared::default());
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        {
            let (stop, shared, token) = (stop.clone(), shared.clone(), token.clone());
            std::thread::Builder::new()
                .name("remote-endpoint".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                let (stop, shared, token, wake) =
                                    (stop.clone(), shared.clone(), token.clone(), wake.clone());
                                let _ = std::thread::Builder::new()
                                    .name("remote-client".into())
                                    .spawn(move || serve(stream, &token, &shared, &stop, &*wake));
                            }
                            Err(_) => std::thread::sleep(Duration::from_millis(50)),
                        }
                    }
                })?;
        }
        Ok(Endpoint { port, token, stop, shared })
    }

    /// What browsers sent since last time.
    pub fn take_inbox(&self) -> Vec<ClientMsg> {
        std::mem::take(&mut *self.shared.inbox.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Whether a browser joined since last asked (it needs a frame now).
    pub fn take_joined(&self) -> bool {
        self.shared.joined.swap(false, Ordering::Relaxed)
    }

    /// How many browsers are looking.
    pub fn clients(&self) -> usize {
        let mut c = self.shared.clients.lock().unwrap_or_else(|e| e.into_inner());
        c.retain(|c| !c.gone.load(Ordering::Acquire));
        c.len()
    }

    /// Whether every browser has caught up (can take another frame). If
    /// not, the next ack wakes the window.
    pub fn ready(&self) -> bool {
        let ready = self.shared.clients.lock().unwrap_or_else(|e| e.into_inner()).iter().all(Client::caught_up);
        self.shared.waiting.store(!ready, Ordering::Release);
        ready
    }

    /// Send to every browser looking at this window.
    pub fn broadcast(&self, out: Out) {
        self.shared.broadcast(out);
    }

    pub fn sender(&self) -> Sender {
        Sender(self.shared.clone())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Ends every link: writers see their channel close, readers their socket.
        self.shared.clients.lock().unwrap_or_else(|e| e.into_inner()).clear();
        for (_, s) in self.shared.sockets.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

/// One browser (through the hub): check the token, then pass messages both
/// ways until either side closes.
// The handshake callback's error type is tungstenite's, sized as it is.
#[allow(clippy::result_large_err)]
fn serve(stream: TcpStream, token: &str, shared: &Shared, stop: &AtomicBool, wake: &dyn Fn()) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let ok = req
            .headers()
            .get(TOKEN_HEADER)
            .is_some_and(|v| v.as_bytes() == token.as_bytes());
        if ok {
            Ok(resp)
        } else {
            let mut deny = ErrorResponse::new(Some("forbidden".into()));
            *deny.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(deny)
        }
    };
    let (Ok(write_half), Ok(socket)) = (stream.try_clone(), stream.try_clone()) else {
        return;
    };
    let Ok(mut reader) = tungstenite::accept_hdr(stream, check) else {
        return;
    };
    // Writing goes on its own thread, through a second WebSocket over the
    // same connection: this one only reads. (The hub never pings, so the
    // reader never writes a reply that could interleave with a frame.)
    let mut writer = tungstenite::WebSocket::from_raw_socket(write_half, Role::Server, None);
    let (tx, rx) = mpsc::channel::<Out>();
    let gone = Arc::new(AtomicBool::new(false));
    let unacked = Arc::new(AtomicU32::new(0));
    let heard = Arc::new(Mutex::new(Instant::now()));
    {
        let gone = gone.clone();
        let spawned = std::thread::Builder::new().name("remote-client-out".into()).spawn(move || {
            for out in rx {
                let sent = match out {
                    Out::Text(t) => writer.send(Message::text(t)),
                    Out::Frame(f) => writer.send(Message::binary(f.to_vec())),
                };
                if sent.is_err() || gone.load(Ordering::Acquire) {
                    break;
                }
            }
            gone.store(true, Ordering::Release);
        });
        if spawned.is_err() {
            return;
        }
    }
    if stop.load(Ordering::Relaxed) {
        return;
    }
    let peer = socket.peer_addr().ok();
    if let Ok(s) = socket.try_clone() {
        shared.sockets.lock().unwrap_or_else(|e| e.into_inner()).push((peer, s));
    }
    shared.clients.lock().unwrap_or_else(|e| e.into_inner()).push(Client {
        tx,
        unacked: unacked.clone(),
        heard: heard.clone(),
        gone: gone.clone(),
    });
    shared.joined.store(true, Ordering::Relaxed);
    wake();
    while !stop.load(Ordering::Relaxed) && !gone.load(Ordering::Acquire) {
        match reader.read() {
            Ok(Message::Text(t)) if t.as_str() == ACK => {
                let _ = unacked.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| Some(n.saturating_sub(1)));
                *heard.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                if shared.waiting.swap(false, Ordering::AcqRel) {
                    wake();
                }
            }
            Ok(Message::Text(t)) => {
                if let Some(msg) = ClientMsg::parse(&t) {
                    shared.inbox.lock().unwrap_or_else(|e| e.into_inner()).push(msg);
                    wake();
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    gone.store(true, Ordering::Release);
    let _ = socket.shutdown(Shutdown::Both);
    // Unblock the writer, waiting on its channel.
    shared.clients.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| !c.gone.load(Ordering::Acquire));
    shared.sockets.lock().unwrap_or_else(|e| e.into_inner()).retain(|(p, _)| *p != peer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect(port: u16, token: &str) -> tungstenite::Result<tungstenite::WebSocket<TcpStream>> {
        use tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://127.0.0.1:{port}/").into_client_request()?;
        req.headers_mut().insert(TOKEN_HEADER, token.parse().unwrap());
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tungstenite::client(req, stream).map(|(ws, _)| ws).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(e) => e,
            tungstenite::HandshakeError::Interrupted(_) => tungstenite::Error::ConnectionClosed,
        })
    }

    #[test]
    fn only_the_hub_gets_in_and_messages_flow_both_ways() {
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = woken.clone();
        let ep = Endpoint::start("tok".into(), move || {
            w.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
        assert!(connect(ep.port, "wrong").is_err(), "a wrong token is refused");

        let mut ws = connect(ep.port, "tok").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while ep.clients() == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ep.take_joined(), "a new browser asks for a frame");
        ws.send(Message::text(r#"{"t":"text","s":"hi"}"#)).unwrap();
        while ep.take_inbox().is_empty() {
            assert!(std::time::Instant::now() < deadline, "never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(woken.load(Ordering::Relaxed) > 0, "the window was woken");
        ep.broadcast(Out::Frame(Arc::new(vec![1, 2, 3])));
        loop {
            match ws.read().unwrap() {
                Message::Binary(b) => {
                    assert_eq!(&b[..], &[1, 2, 3]);
                    break;
                }
                _ => continue,
            }
        }
        // Flow control: two frames unacked and it waits; an ack wakes the
        // window and lets the next one go.
        assert!(ep.ready());
        ep.broadcast(Out::Frame(Arc::new(vec![4])));
        assert!(!ep.ready(), "two frames in flight");
        let before = woken.load(Ordering::Relaxed);
        ws.send(Message::text(ACK)).unwrap();
        while !ep.ready() {
            assert!(std::time::Instant::now() < deadline, "the ack never counted");
            std::thread::sleep(Duration::from_millis(5));
        }
        while woken.load(Ordering::Relaxed) == before {
            assert!(std::time::Instant::now() < deadline, "the ack didn't wake the waiting window");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ep.take_inbox().is_empty(), "an ack isn't input");
        drop(ws);
        while ep.clients() != 0 {
            assert!(std::time::Instant::now() < deadline, "a closed browser is forgotten");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
