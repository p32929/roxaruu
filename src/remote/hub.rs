//! The server on the Wi-Fi address: one per Mac, run by the window where
//! remote access was turned on, for as long as it stays on there (it stops
//! with that window; nobody takes over). It serves the login and the page,
//! lists every window open to remote access, and links a browser to the
//! window it's looking at — over HTTPS, after checking the login and that
//! the request comes from the page itself. Stopping it ends every link.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustls::{ServerConfig, ServerConnection, StreamOwned};
use tungstenite::Message;

use super::auth::{SESSION_SECS, Secrets};
use super::endpoint::TOKEN_HEADER;
use super::http::{Request, form_field, respond};
use crate::instances::RemoteWindow;

const COOKIE: &str = "rox";
const PAGE: &str = include_str!("page.html");
const LOGIN: &str = include_str!("login.html");

/// What the hub needs to know, swappable for tests.
pub struct Setup {
    pub port: u16,
    /// The password hash and cookie key (in memory only).
    pub secrets: Secrets,
    pub tls: Arc<ServerConfig>,
    /// The windows open to remote access.
    pub windows: fn() -> Vec<RemoteWindow>,
}

pub struct Hub {
    stop: Arc<AtomicBool>,
}

impl Hub {
    /// Take the port and serve until dropped.
    pub fn start(setup: Setup) -> std::io::Result<Hub> {
        let listener = TcpListener::bind(("0.0.0.0", setup.port))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        std::thread::Builder::new()
            .name("remote-hub".into())
            .spawn(move || run(listener, setup, s))?;
        Ok(Hub { stop })
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run(listener: TcpListener, setup: Setup, stop: Arc<AtomicBool>) {
    let setup = Arc::new(setup);
    let failures = Arc::new(AtomicU32::new(0));
    // Returning drops the listener: the port is free again.
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((tcp, _)) => {
                let (setup, failures, stop) = (setup.clone(), failures.clone(), stop.clone());
                let _ = std::thread::Builder::new()
                    .name("remote-request".into())
                    .spawn(move || handle(tcp, &setup, &failures, &stop));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(40)),
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

type Tls = StreamOwned<ServerConnection, TcpStream>;

fn handle(tcp: TcpStream, setup: &Setup, failures: &AtomicU32, stop: &AtomicBool) {
    let from = tcp.peer_addr().ok();
    let _ = tcp.set_nonblocking(false);
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(20)));
    let Ok(conn) = ServerConnection::new(setup.tls.clone()) else {
        return;
    };
    let mut tls: Tls = StreamOwned::new(conn, tcp);
    let Some(mut req) = Request::read(&mut tls) else {
        return;
    };
    let secrets = &setup.secrets;
    let signed_in = req.cookie(COOKIE).is_some_and(|t| secrets.check(t, now()));
    let path = req.path.split('?').next().unwrap_or("/").to_string();
    let html = [("Content-Type", "text/html; charset=utf-8")];
    let _ = match (req.method.as_str(), path.as_str()) {
        ("GET", "/") if signed_in => respond(&mut tls, "200 OK", &html, PAGE.as_bytes()),
        ("GET", "/") => {
            let wrong = req.path.contains("wrong");
            let page = LOGIN.replace("{{ERROR}}", if wrong { "That password didn't work." } else { "" });
            respond(&mut tls, "200 OK", &html, page.as_bytes())
        }
        ("POST", "/login") => {
            let body = req.body(&mut tls, 8 * 1024).unwrap_or_default();
            let password = form_field(&body, "password").unwrap_or_default();
            if secrets.verify(&password) {
                tracing::info!(?from, "remote sign-in");
                failures.store(0, Ordering::Relaxed);
                let cookie = format!(
                    "{COOKIE}={}; Path=/; Max-Age={SESSION_SECS}; HttpOnly; Secure; SameSite=Strict",
                    secrets.issue(now())
                );
                respond(&mut tls, "303 See Other", &[("Location", "/"), ("Set-Cookie", &cookie)], b"")
            } else {
                // Slow guessing down: longer with each miss in a row.
                let n = failures.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::warn!(?from, misses = n, "remote sign-in: wrong password");
                std::thread::sleep(Duration::from_millis(500 * u64::from(n.min(10))));
                respond(&mut tls, "303 See Other", &[("Location", "/?wrong")], b"")
            }
        }
        ("POST", "/logout") => respond(
            &mut tls,
            "303 See Other",
            &[
                ("Location", "/"),
                ("Set-Cookie", &format!("{COOKIE}=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Strict")),
            ],
            b"",
        ),
        ("GET", "/api/windows") if signed_in => {
            let list: Vec<serde_json::Value> = (setup.windows)()
                .into_iter()
                .map(|w| serde_json::json!({ "id": w.pid, "name": w.name, "path": w.path }))
                .collect();
            respond(
                &mut tls,
                "200 OK",
                &[("Content-Type", "application/json")],
                serde_json::Value::Array(list).to_string().as_bytes(),
            )
        }
        ("GET", p) if p.starts_with("/ws/") && signed_in && req.is_websocket() && same_origin(&req) => {
            let id = p.trim_start_matches("/ws/").parse::<i32>().ok();
            match (setup.windows)().into_iter().find(|w| Some(w.pid) == id) {
                Some(w) => {
                    link(tls, req, &w, stop);
                    return;
                }
                None => respond(&mut tls, "404 Not Found", &[], b"no such window"),
            }
        }
        ("GET", p) if p == "/api/windows" || p.starts_with("/ws/") => {
            respond(&mut tls, "401 Unauthorized", &[], b"sign in first")
        }
        _ => respond(&mut tls, "404 Not Found", &[], b"not found"),
    };
}

/// A live link must come from our own page: its `Origin` is this server.
fn same_origin(req: &Request) -> bool {
    match (req.header("origin"), req.header("host")) {
        (Some(origin), Some(host)) => origin == format!("https://{host}"),
        _ => false,
    }
}

/// The request head already read, then the rest of the stream: lets the
/// WebSocket handshake read a request we've looked at.
struct Replay<S> {
    head: std::io::Cursor<Vec<u8>>,
    inner: S,
}

impl<S: Read> Read for Replay<S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.head.read(buf)?;
        if n > 0 { Ok(n) } else { self.inner.read(buf) }
    }
}

impl<S: Write> Write for Replay<S> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// One TLS connection, shared by a thread reading it and one writing it
/// (rustls's state behind a lock; the socket itself is read without it, so
/// a waiting read never holds up a write).
#[derive(Clone)]
struct TlsShare {
    conn: Arc<std::sync::Mutex<ServerConnection>>,
    tcp: Arc<TcpStream>,
}

impl TlsShare {
    fn lock(&self) -> std::sync::MutexGuard<'_, ServerConnection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Send whatever TLS records are waiting (with the lock held).
    fn push(conn: &mut ServerConnection, mut tcp: &TcpStream) -> std::io::Result<()> {
        while conn.wants_write() {
            conn.write_tls(&mut tcp)?;
        }
        Ok(())
    }
}

impl Read for TlsShare {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.lock().reader().read(buf) {
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                done => return done,
            }
            let mut raw = [0u8; 16 * 1024];
            let n = (&*self.tcp).read(&mut raw)?;
            if n == 0 {
                return Ok(0);
            }
            let mut conn = self.lock();
            let mut rest = &raw[..n];
            while !rest.is_empty() {
                if conn.read_tls(&mut rest)? == 0 {
                    break;
                }
                conn.process_new_packets().map_err(std::io::Error::other)?;
            }
            Self::push(&mut conn, &self.tcp)?;
        }
    }
}

impl Write for TlsShare {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut conn = self.lock();
        let n = conn.writer().write(buf)?;
        Self::push(&mut conn, &self.tcp)?;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let mut conn = self.lock();
        conn.writer().flush()?;
        Self::push(&mut conn, &self.tcp)
    }
}

/// Join the browser to the window: pass messages both ways until one side
/// goes or the hub stops. A thread for each direction, each waiting on its
/// own socket, so a frame or a key goes on the moment it arrives.
fn link(tls: Tls, req: Request, window: &RemoteWindow, stop: &AtomicBool) {
    use tungstenite::client::IntoClientRequest;
    use tungstenite::protocol::{Role, WebSocket};
    let StreamOwned { conn, sock } = tls;
    let _ = sock.set_read_timeout(None);
    let _ = sock.set_nodelay(true);
    let share = TlsShare { conn: Arc::new(std::sync::Mutex::new(conn)), tcp: Arc::new(sock) };
    let mut head = req.raw_head.clone();
    head.extend_from_slice(&req.rest);
    let Ok(mut from_browser) =
        tungstenite::accept(Replay { head: std::io::Cursor::new(head), inner: share.clone() })
    else {
        return;
    };
    let mut to_browser = WebSocket::from_raw_socket(share.clone(), Role::Server, None);
    let Ok(mut to_window) = format!("ws://127.0.0.1:{}/", window.port).into_client_request() else {
        return;
    };
    let Ok(token) = window.token.parse() else {
        return;
    };
    to_window.headers_mut().insert(TOKEN_HEADER, token);
    let tcp = match TcpStream::connect(("127.0.0.1", window.port)) {
        Ok(tcp) => tcp,
        Err(_) => {
            let _ = from_browser.close(None);
            return;
        }
    };
    let _ = tcp.set_nodelay(true);
    let (Ok(win_write), Ok(win_socket)) = (tcp.try_clone(), tcp.try_clone()) else {
        return;
    };
    let Ok((mut from_window, _)) = tungstenite::client(to_window, tcp) else {
        let _ = from_browser.close(None);
        return;
    };
    let mut to_win = WebSocket::from_raw_socket(win_write, Role::Client, None);
    let done = Arc::new(AtomicBool::new(false));
    let relay = |from: &mut dyn FnMut() -> tungstenite::Result<Message>,
                 to: &mut dyn FnMut(Message) -> tungstenite::Result<()>,
                 done: &AtomicBool| {
        while !done.load(Ordering::Relaxed) {
            match from() {
                Ok(m @ (Message::Text(_) | Message::Binary(_))) => {
                    if to(m).is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
        done.store(true, Ordering::Relaxed);
    };
    std::thread::scope(|s| {
        let d = done.clone();
        s.spawn(move || relay(&mut || from_browser.read(), &mut |m| to_win.send(m), &d));
        let d = done.clone();
        s.spawn(move || relay(&mut || from_window.read(), &mut |m| to_browser.send(m), &d));
        // Both wait on their sockets: closing those ends them, when either
        // side goes or the hub stops.
        while !done.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
        done.store(true, Ordering::Relaxed);
        let _ = share.tcp.shutdown(std::net::Shutdown::Both);
        let _ = win_socket.shutdown(std::net::Shutdown::Both);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::endpoint::{Endpoint, Out};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    /// The hub takes a plain `fn` for its window list; tests fill this.
    fn test_windows() -> Vec<RemoteWindow> {
        WINDOWS.get().map(|w| w.lock().unwrap().clone()).unwrap_or_default()
    }
    static WINDOWS: OnceLock<Mutex<Vec<RemoteWindow>>> = OnceLock::new();

    use crate::remote::testing::{https, tls_client};

    #[test]
    fn sign_in_list_windows_and_link_to_one() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Secrets::new("open sesame").unwrap();
        let tls = crate::remote::tls::server_config(
            &dir.path().join("tls"),
            &["localhost".into(), "127.0.0.1".into()],
        )
        .unwrap();
        let window = Endpoint::start("win-token".into(), || {}).unwrap();
        WINDOWS.get_or_init(Default::default).lock().unwrap().push(RemoteWindow {
            pid: 4242,
            port: window.port,
            token: "win-token".into(),
            name: "shop".into(),
            path: "~/code/shop".into(),
        });
        let port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let hub = Hub::start(Setup { port, secrets, tls, windows: test_windows }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "the hub never started");
            std::thread::sleep(Duration::from_millis(50));
        }
        let host = format!("localhost:{port}");

        // Signed out: the login page, and nothing else.
        let page = https(port, &format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n"));
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("type=\"password\""), "{page}");
        assert!(page.contains("X-Frame-Options: DENY"), "never shown inside another site's frame");
        let api = https(port, &format!("GET /api/windows HTTP/1.1\r\nHost: {host}\r\n\r\n"));
        assert!(api.starts_with("HTTP/1.1 401"), "{api}");

        // A wrong password, then the right one.
        let login = |pw: &str| {
            let body = format!("password={pw}");
            https(
                port,
                &format!(
                    "POST /login HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/x-www-form-urlencoded\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    body.len()
                ),
            )
        };
        let wrong = login("nope");
        assert!(wrong.contains("Location: /?wrong") && !wrong.contains("Set-Cookie"), "{wrong}");
        let right = login("open+sesame");
        let cookie = right
            .lines()
            .find_map(|l| l.strip_prefix("Set-Cookie: "))
            .expect("a session cookie");
        assert!(cookie.contains("HttpOnly") && cookie.contains("Secure") && cookie.contains("SameSite=Strict"));
        let session = cookie.split(';').next().unwrap().to_string();

        // Signed in: the page, and the window list.
        let page = https(port, &format!("GET / HTTP/1.1\r\nHost: {host}\r\nCookie: {session}\r\n\r\n"));
        assert!(page.contains("<canvas"), "the app page");
        let api = https(port, &format!("GET /api/windows HTTP/1.1\r\nHost: {host}\r\nCookie: {session}\r\n\r\n"));
        assert!(api.contains("\"name\":\"shop\"") && api.contains("\"id\":4242"), "{api}");
        assert!(!api.contains("win-token"), "a window's token never reaches the browser");

        // A live link from somewhere else (no matching Origin) is refused.
        let foreign = https(
            port,
            &format!(
                "GET /ws/4242 HTTP/1.1\r\nHost: {host}\r\nCookie: {session}\r\nUpgrade: websocket\r\n\
                 Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                 Sec-WebSocket-Version: 13\r\nOrigin: https://evil.example\r\n\r\n"
            ),
        );
        assert!(!foreign.starts_with("HTTP/1.1 101"), "{foreign}");

        // From the page: linked to the window, both ways.
        use tungstenite::client::IntoClientRequest;
        let mut req = format!("wss://{host}/ws/4242").into_client_request().unwrap();
        req.headers_mut().insert("Cookie", session.parse().unwrap());
        req.headers_mut().insert("Origin", format!("https://{host}").parse().unwrap());
        let (mut ws, resp) = tungstenite::client(req, tls_client(port)).unwrap();
        assert_eq!(resp.status(), 101);
        ws.send(Message::text(r#"{"t":"text","s":"hello"}"#)).unwrap();
        loop {
            let got = window.take_inbox();
            if !got.is_empty() {
                assert_eq!(got[0], crate::remote::input::ClientMsg::Text { s: "hello".into() });
                break;
            }
            assert!(Instant::now() < deadline, "the window never heard the browser");
            std::thread::sleep(Duration::from_millis(20));
        }
        window.broadcast(Out::Frame(Arc::new(vec![0xFF, 0xD8, 7])));
        loop {
            match ws.read().unwrap() {
                Message::Binary(b) => {
                    assert_eq!(&b[..], &[0xFF, 0xD8, 7]);
                    break;
                }
                _ => assert!(Instant::now() < deadline),
            }
        }

        // Stopping the hub ends the live link and frees the port.
        drop(hub);
        while ws.read().is_ok() {
            assert!(Instant::now() < deadline, "the link outlived the hub");
        }
        while TcpStream::connect(("127.0.0.1", port)).is_ok() {
            assert!(Instant::now() < deadline, "still listening after stopping");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_taken_port_is_an_error_not_a_wait() {
        let dir = tempfile::tempdir().unwrap();
        let busy = TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let port = busy.local_addr().unwrap().port();
        let tls = crate::remote::tls::server_config(&dir.path().join("tls"), &["localhost".into()]).unwrap();
        let secrets = Secrets::new("pw").unwrap();
        assert!(Hub::start(Setup { port, secrets, tls, windows: test_windows }).is_err());
    }
}
