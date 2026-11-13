//! A browser, for tests: trusts the hub's self-signed certificate, signs
//! in, and opens a live link to a window.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use rustls::StreamOwned;

/// Accepts any certificate (the hub's own self-signed one, in tests).
#[derive(Debug)]
struct AnyCert;

impl rustls::client::danger::ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub type Tls = StreamOwned<rustls::ClientConnection, TcpStream>;

pub fn tls_client(port: u16) -> Tls {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(AnyCert))
    .with_no_client_auth();
    let conn = rustls::ClientConnection::new(Arc::new(config), "localhost".try_into().unwrap()).unwrap();
    let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
    StreamOwned::new(conn, tcp)
}

/// Send a raw request, return the whole response.
pub fn https(port: u16, request: &str) -> String {
    let mut s = tls_client(port);
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// Sign in with `password`; the session cookie (`rox=…`), if it worked.
pub fn sign_in(port: u16, password: &str) -> Option<String> {
    let body = format!("password={}", password.replace(' ', "+"));
    let resp = https(
        port,
        &format!(
            "POST /login HTTP/1.1\r\nHost: localhost:{port}\r\n\
             Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    let cookie = resp.lines().find_map(|l| l.strip_prefix("Set-Cookie: "))?;
    Some(cookie.split(';').next()?.to_string())
}

/// A live link to window `id`, as the page opens it.
pub fn link(port: u16, id: i32, session: &str) -> tungstenite::WebSocket<Tls> {
    use tungstenite::client::IntoClientRequest;
    let host = format!("localhost:{port}");
    let mut req = format!("wss://{host}/ws/{id}").into_client_request().unwrap();
    req.headers_mut().insert("Cookie", session.parse().unwrap());
    req.headers_mut().insert("Origin", format!("https://{host}").parse().unwrap());
    tungstenite::client(req, tls_client(port)).unwrap().0
}
