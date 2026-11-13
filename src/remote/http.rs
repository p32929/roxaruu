//! Just enough HTTP/1.1 for the remote page: read a request's head, answer
//! with a status, headers and body. Each request gets its own connection
//! (`Connection: close`), which keeps this small.

use std::collections::HashMap;
use std::io::{Read, Write};

/// A request head, with whatever body bytes arrived with it.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// Lower-cased names.
    pub headers: HashMap<String, String>,
    /// Bytes after the head that were read with it.
    pub rest: Vec<u8>,
    /// The head exactly as received (for passing on).
    pub raw_head: Vec<u8>,
}

/// Heads larger than this are refused.
const MAX_HEAD: usize = 16 * 1024;

impl Request {
    /// Read a request head from `r`. `None` for a closed or malformed one.
    pub fn read(r: &mut impl Read) -> Option<Request> {
        let mut buf = Vec::with_capacity(1024);
        let mut chunk = [0u8; 2048];
        let end = loop {
            if let Some(i) = find(&buf, b"\r\n\r\n") {
                break i + 4;
            }
            if buf.len() > MAX_HEAD {
                return None;
            }
            let n = r.read(&mut chunk).ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(chunk.get(..n)?);
        };
        let (head_bytes, rest) = buf.split_at_checked(end)?;
        let head = std::str::from_utf8(head_bytes).ok()?;
        let mut lines = head.split("\r\n");
        let mut first = lines.next()?.split(' ');
        let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        Some(Request {
            method,
            path,
            headers,
            rest: rest.to_vec(),
            raw_head: head_bytes.to_vec(),
        })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// A cookie's value.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?
            .split(';')
            .filter_map(|c| c.trim().split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    }

    /// The whole body (`Content-Length` bytes, up to `max`).
    pub fn body(&mut self, r: &mut impl Read, max: usize) -> Option<Vec<u8>> {
        let len: usize = self.header("content-length")?.parse().ok()?;
        if len > max {
            return None;
        }
        let mut body = std::mem::take(&mut self.rest);
        while body.len() < len {
            let mut chunk = vec![0u8; len - body.len()];
            let n = r.read(&mut chunk).ok()?;
            if n == 0 {
                return None;
            }
            body.extend_from_slice(chunk.get(..n)?);
        }
        body.truncate(len);
        Some(body)
    }

    pub fn is_websocket(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A field of an `application/x-www-form-urlencoded` body.
pub fn form_field(body: &[u8], name: &str) -> Option<String> {
    let body = std::str::from_utf8(body).ok()?;
    body.split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| url_decode(v))
}

/// `+` and `%XX` decoding (bytes, so any UTF-8 survives).
pub fn url_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (c, hex) {
            (b'+', _) => out.push(b' '),
            (b'%', Some(v)) => {
                out.push(v);
                i += 2;
            }
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Write a whole response and close.
pub fn respond(
    w: &mut impl Write,
    status: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
         X-Frame-Options: DENY\r\nContent-Security-Policy: frame-ancestors 'none'\r\n\r\n",
        body.len()
    ));
    w.write_all(head.as_bytes())?;
    w.write_all(body)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_head_split_across_reads() {
        // A reader that hands out a few bytes at a time.
        struct Drip(Vec<u8>, usize);
        impl Read for Drip {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = (self.0.len() - self.1).min(7).min(buf.len());
                buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
                self.1 += n;
                Ok(n)
            }
        }
        let raw = b"POST /login HTTP/1.1\r\nHost: 10.0.0.2:7878\r\nCookie: a=1; rox=abc.def\r\n\
                    Content-Length: 15\r\n\r\npassword=x+y%21".to_vec();
        let mut r = Drip(raw, 0);
        let mut req = Request::read(&mut r).unwrap();
        assert_eq!((req.method.as_str(), req.path.as_str()), ("POST", "/login"));
        assert_eq!(req.header("host"), Some("10.0.0.2:7878"));
        assert_eq!(req.cookie("rox"), Some("abc.def"));
        assert_eq!(req.cookie("nope"), None);
        let body = req.body(&mut r, 1024).unwrap();
        assert_eq!(form_field(&body, "password").as_deref(), Some("x y!"));
    }

    #[test]
    fn refuses_junk_and_huge_heads() {
        assert!(Request::read(&mut &b""[..]).is_none());
        let huge = vec![b'a'; MAX_HEAD + 4096];
        assert!(Request::read(&mut &huge[..]).is_none());
    }

    #[test]
    fn decodes_forms() {
        assert_eq!(url_decode("caf%C3%A9+%F0%9F%90%8E"), "café 🐎");
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(url_decode("%zz"), "%zz");
    }
}
