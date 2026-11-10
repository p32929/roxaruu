//! HTTPS for remote access: a self-signed certificate, made once and kept in
//! the config folder (so the browser's "proceed anyway" sticks), and made
//! again only when the Mac's network names change (a new Wi-Fi address).

use std::path::Path;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// The certificate and key for `names` (IPs and host names), from `dir`,
/// made there first if missing or made for other names.
pub fn server_config(dir: &Path, names: &[String]) -> Result<Arc<ServerConfig>, String> {
    let (cert_path, key_path, names_path) = (
        dir.join("cert.pem"),
        dir.join("key.pem"),
        dir.join("names.txt"),
    );
    let wanted = names.join("\n");
    let current = std::fs::read_to_string(&names_path).unwrap_or_default();
    if current != wanted || !cert_path.exists() || !key_path.exists() {
        let made = rcgen::generate_simple_self_signed(names.to_vec())
            .map_err(|e| format!("couldn't make a certificate: {e}"))?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        write_private(&key_path, made.signing_key.serialize_pem().as_bytes())?;
        std::fs::write(&cert_path, made.cert.pem()).map_err(|e| e.to_string())?;
        std::fs::write(&names_path, wanted).map_err(|e| e.to_string())?;
    }
    let cert = CertificateDer::from_pem_file(&cert_path).map_err(|e| e.to_string())?;
    let key = PrivateKeyDer::from_pem_file(&key_path).map_err(|e| e.to_string())?;
    let config = ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .map_err(|e| e.to_string())?;
    Ok(Arc::new(config))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_once_then_reused_until_the_names_change() {
        let dir = tempfile::tempdir().unwrap();
        let names = vec!["192.168.1.20".to_string(), "localhost".to_string()];
        server_config(dir.path(), &names).unwrap();
        let first = std::fs::read(dir.path().join("cert.pem")).unwrap();
        server_config(dir.path(), &names).unwrap();
        assert_eq!(std::fs::read(dir.path().join("cert.pem")).unwrap(), first, "kept");
        let moved = vec!["10.0.0.7".to_string(), "localhost".to_string()];
        server_config(dir.path(), &moved).unwrap();
        assert_ne!(std::fs::read(dir.path().join("cert.pem")).unwrap(), first, "new address, new cert");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("key.pem")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the key is private");
        }
    }
}
