//! The bundled fonts go into the binary deflated (7 MB → about 4): they're
//! inflated once, when first used (`fonts.rs`).

use std::io::Write;

fn main() {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=assets/fonts");
    for entry in std::fs::read_dir("assets/fonts").unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "ttf") {
            continue;
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(&std::fs::read(&path).unwrap()).unwrap();
        let name = format!("{}.z", path.file_name().unwrap().to_string_lossy());
        std::fs::write(out.join(name), z.finish().unwrap()).unwrap();
    }
}
