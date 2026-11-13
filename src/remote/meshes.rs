//! The browser draws: instead of a picture, a frame is what egui would draw
//! (its triangle meshes and the textures they use) for the page to draw with
//! WebGL, on the GPU of the device it's on. This computer then only lays the
//! window out (a few ms): one without a GPU, which draws on its CPU (hundreds
//! of ms a frame), serves as fast as one with. Pages without WebGL get
//! pictures (`frames.rs`).
//!
//! One message is one frame, zlib-compressed after its first byte (browsers
//! inflate it natively):
//!
//! ```text
//! u8 2 | zlib(
//!   u16 width, u16 height (pixels) | f32 pixels per point
//!   u16 count × texture set: u8 kind, u64 id | u8 patch | (u32 x, y if patch)
//!                            | u32 w, h | u8 mag, min (0 linear, 1 nearest), wrap
//!                            | w×h×4 RGBA (premultiplied sRGB)
//!   u16 count × texture freed: u8 kind, u64 id
//!   u32 count × mesh: f32 clip min x, y, max x, y (points) | u8 kind, u64 id
//!                     | u32 vertices, u32 indices | u8 index bytes (2 or 4)
//!                     | vertices × (f32 x, y (points), f32 u, v, u8 r, g, b, a)
//!                     | indices
//! )
//! ```
//!
//! (little-endian; `kind` 0 is egui's own texture, 1 a user one.) A frame
//! for a browser that just joined carries every texture whole.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, ColorImage, TextureFilter, TextureId, TextureOptions, TextureWrapMode, TexturesDelta};
use eframe::epaint::{ClippedPrimitive, ImageData, Primitive};

/// The first byte of a mesh frame (pictures are 1).
pub const KIND: u8 = 2;

/// Every texture the browser's copy uses, as egui last set it: what a
/// browser that joins later is sent whole.
#[derive(Default)]
pub struct Textures(HashMap<TextureId, (TextureOptions, Arc<ColorImage>)>);

impl Textures {
    /// Apply a pass's texture changes.
    pub fn apply(&mut self, delta: &TexturesDelta) {
        for (id, d) in delta.set.iter().flat_map(|(id, ds)| ds.iter().map(move |d| (id, d))) {
            let ImageData::Color(image) = &d.image;
            match d.pos {
                None => {
                    self.0.insert(*id, (d.options, image.clone()));
                }
                Some([x, y]) => {
                    let Some((_, whole)) = self.0.get_mut(id) else { continue };
                    let whole = Arc::make_mut(whole);
                    let [w, h] = image.size;
                    for row in 0..h {
                        let at = (y + row) * whole.size[0] + x;
                        if let Some(to) = whole.pixels.get_mut(at..at + w) {
                            to.copy_from_slice(&image.pixels[row * w..(row + 1) * w]);
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.0.remove(id);
        }
    }
}

impl Textures {
    /// Every texture, as changes that set each one whole (a renderer
    /// starting afresh).
    pub fn all(&self) -> TexturesDelta {
        let mut delta = TexturesDelta::default();
        for (tex, (options, image)) in &self.0 {
            delta
                .set
                .entry(*tex)
                .or_default()
                .push(egui::epaint::ImageDelta::full(ImageData::Color(image.clone()), *options));
        }
        delta
    }
}

fn id(out: &mut Vec<u8>, id: TextureId) {
    let (kind, n) = match id {
        TextureId::Managed(n) => (0u8, n),
        TextureId::User(n) => (1, n),
    };
    out.push(kind);
    out.extend_from_slice(&n.to_le_bytes());
}

fn u32(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u32).to_le_bytes());
}

fn texture(out: &mut Vec<u8>, tex: TextureId, pos: Option<[usize; 2]>, options: TextureOptions, image: &ColorImage) {
    id(out, tex);
    out.push(u8::from(pos.is_some()));
    if let Some([x, y]) = pos {
        u32(out, x);
        u32(out, y);
    }
    u32(out, image.size[0]);
    u32(out, image.size[1]);
    let filter = |f| u8::from(f == TextureFilter::Nearest);
    out.push(filter(options.magnification));
    out.push(filter(options.minification));
    out.push(match options.wrap_mode {
        TextureWrapMode::ClampToEdge => 0,
        TextureWrapMode::Repeat => 1,
        TextureWrapMode::MirroredRepeat => 2,
    });
    out.reserve(image.pixels.len() * 4);
    for p in &image.pixels {
        out.extend_from_slice(&p.to_array());
    }
}

/// The textures part of a frame: every one whole (`all`, for a browser that
/// just joined), or what changed.
pub fn textures(all: Option<&Textures>, delta: &TexturesDelta) -> Vec<u8> {
    let mut out = Vec::new();
    match all {
        Some(all) => {
            out.extend_from_slice(&(all.0.len() as u16).to_le_bytes());
            for (tex, (options, image)) in &all.0 {
                texture(&mut out, *tex, None, *options, image);
            }
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        None => {
            // In order: a texture's whole image, then its patches.
            out.extend_from_slice(&(delta.set.values().map(|ds| ds.len()).sum::<usize>() as u16).to_le_bytes());
            for (tex, ds) in &delta.set {
                for d in ds {
                    let ImageData::Color(image) = &d.image;
                    texture(&mut out, *tex, d.pos, d.options, image);
                }
            }
            out.extend_from_slice(&(delta.free.len() as u16).to_le_bytes());
            for tex in &delta.free {
                id(&mut out, *tex);
            }
        }
    }
    out
}

/// The meshes part of a frame.
pub fn meshes(primitives: &[ClippedPrimitive]) -> Vec<u8> {
    let mut out = Vec::new();
    let meshes: Vec<_> = primitives
        .iter()
        .filter_map(|p| match &p.primitive {
            Primitive::Mesh(m) if !m.indices.is_empty() => Some((p.clip_rect, m)),
            _ => None, // ponytail: paint callbacks aren't sent; Roxaruu has none
        })
        .collect();
    u32(&mut out, meshes.len());
    for (clip, m) in meshes {
        for v in [clip.min.x, clip.min.y, clip.max.x, clip.max.y] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        id(&mut out, m.texture_id);
        u32(&mut out, m.vertices.len());
        u32(&mut out, m.indices.len());
        let wide = m.vertices.len() > usize::from(u16::MAX);
        out.push(if wide { 4 } else { 2 });
        out.reserve(m.vertices.len() * 20 + m.indices.len() * 4);
        for v in &m.vertices {
            for f in [v.pos.x, v.pos.y, v.uv.x, v.uv.y] {
                out.extend_from_slice(&f.to_le_bytes());
            }
            out.extend_from_slice(&v.color.to_array());
        }
        for &i in &m.indices {
            if wide {
                out.extend_from_slice(&i.to_le_bytes());
            } else {
                out.extend_from_slice(&(i as u16).to_le_bytes());
            }
        }
    }
    out
}

/// A whole frame, before compression: its size, then `textures` and `meshes`.
pub fn frame(size: egui::Vec2, ppp: f32, textures: &[u8], meshes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + textures.len() + meshes.len());
    out.extend_from_slice(&(size.x.round().max(1.0) as u16).to_le_bytes());
    out.extend_from_slice(&(size.y.round().max(1.0) as u16).to_le_bytes());
    out.extend_from_slice(&ppp.to_le_bytes());
    out.extend_from_slice(textures);
    out.extend_from_slice(meshes);
    out
}

/// The message: the kind, then the frame compressed.
pub fn compress(frame: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(vec![KIND], flate2::Compression::fast());
    let _ = z.write_all(frame);
    z.finish().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn a_patch_lands_in_the_whole_texture_and_a_join_gets_it_all() {
        let tex = TextureId::Managed(0);
        let whole = ColorImage::filled([4, 3], egui::Color32::BLACK);
        let mut all = Textures::default();
        let mut delta = TexturesDelta::default();
        delta.set.entry(tex).or_default().push(egui::epaint::ImageDelta::full(whole, TextureOptions::LINEAR));
        all.apply(&delta);
        let patch = ColorImage::filled([2, 1], egui::Color32::WHITE);
        let mut delta = TexturesDelta::default();
        delta.set.entry(tex).or_default().push(egui::epaint::ImageDelta::partial([1, 2], patch, TextureOptions::LINEAR));
        all.apply(&delta);
        let (_, image) = &all.0[&tex];
        let white: Vec<_> = image.pixels.iter().map(|p| *p == egui::Color32::WHITE).collect();
        assert_eq!(white, [[false; 4], [false; 4], [false, true, true, false]].concat());
        // Joining: one texture, whole (no patch), 4×3, its pixels after.
        let t = textures(Some(&all), &TexturesDelta::default());
        assert_eq!(u16::from_le_bytes([t[0], t[1]]), 1);
        assert_eq!(t[2 + 9], 0, "whole, not a patch");
        assert_eq!(&t[12..20], &[4, 0, 0, 0, 3, 0, 0, 0]);
        assert_eq!(t.len(), 2 + 9 + 1 + 8 + 3 + 4 * 3 * 4 + 2);
        let mut freed = TexturesDelta::default();
        freed.free.insert(tex);
        all.apply(&freed);
        assert!(all.0.is_empty());
    }

    #[test]
    fn meshes_round_trip_through_the_message() {
        let mut mesh = egui::Mesh::default();
        mesh.add_colored_rect(egui::Rect::from_min_size(egui::pos2(1.0, 2.0), egui::vec2(3.0, 4.0)), egui::Color32::RED);
        let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 50.0));
        let m = meshes(&[ClippedPrimitive { clip_rect: clip, primitive: Primitive::Mesh(mesh) }]);
        assert_eq!(u32::from_le_bytes(m[..4].try_into().unwrap()), 1);
        let body = &m[4 + 16 + 9..];
        assert_eq!(u32::from_le_bytes(body[..4].try_into().unwrap()), 4, "vertices");
        assert_eq!(u32::from_le_bytes(body[4..8].try_into().unwrap()), 6, "indices");
        assert_eq!(body[8], 2, "16-bit indices");
        assert_eq!(body.len(), 9 + 4 * 20 + 6 * 2);
        let msg = compress(&frame(egui::vec2(200.0, 100.0), 2.0, &textures(None, &TexturesDelta::default()), &m));
        assert_eq!(msg[0], KIND);
        let mut back = Vec::new();
        flate2::read::ZlibDecoder::new(&msg[1..]).read_to_end(&mut back).unwrap();
        assert_eq!(&back[..8], [200u16.to_le_bytes(), 100u16.to_le_bytes()].concat().iter().chain(&2f32.to_le_bytes()).copied().collect::<Vec<_>>());
    }
}
