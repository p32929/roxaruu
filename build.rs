//! The bundled fonts go into the binary deflated (7 MB → about 4): they're
//! inflated once, when first used (`fonts.rs`). On Windows the app's icon
//! goes into the .exe itself, so every shortcut, Explorer, the taskbar and
//! the right-click entries show it, however it was installed.

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR")?);
    println!("cargo:rerun-if-changed=assets/fonts");
    for entry in std::fs::read_dir("assets/fonts")?.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        if !name.ends_with(".ttf") {
            continue;
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(&std::fs::read(&path)?)?;
        std::fs::write(out.join(format!("{name}.z")), z.finish()?)?;
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new().set_icon("assets/icon.ico").compile()?;
    }
    Ok(())
}
