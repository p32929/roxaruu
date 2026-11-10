//! Draws the browser's copy of a window off-screen, on the app's own GPU
//! device, and hands back the pixels. Off-screen means it works with the Mac
//! window covered, minimized or the screen locked.

use eframe::egui;
use egui_wgpu::wgpu;

/// How long to wait for the GPU before giving up on a frame.
const GPU_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub struct Offscreen {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    /// The target and its readback buffer, kept while the size stays.
    target: Option<Target>,
}

struct Target {
    size: (u32, u32),
    texture: wgpu::Texture,
    buffer: wgpu::Buffer,
}

/// RGBA pixels, top row first.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Offscreen {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Offscreen {
        Offscreen {
            device: device.clone(),
            queue: queue.clone(),
            renderer: egui_wgpu::Renderer::new(device, FORMAT, egui_wgpu::RendererOptions::default()),
            target: None,
        }
    }

    /// Draw `output` (from `ctx`'s last pass) and read it back.
    pub fn render(&mut self, ctx: &egui::Context, output: &egui::FullOutput) -> Option<Image> {
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        let ppp = ctx.pixels_per_point();
        let size = ctx.content_rect().size() * ppp;
        let (width, height) = (size.x.round().max(1.0) as u32, size.y.round().max(1.0) as u32);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: ppp,
        };
        let jobs = ctx.tessellate(output.shapes.clone(), ppp);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("remote") });
        let user = self
            .renderer
            .update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        // Rows padded to the GPU's copy alignment.
        let unpadded = width as usize * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded = unpadded.div_ceil(align) * align;
        if self.target.as_ref().is_none_or(|t| t.size != (width, height)) {
            self.target = Some(Target {
                size: (width, height),
                texture: self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("remote frame"),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                }),
                buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("remote readback"),
                    size: (padded * height as usize) as u64,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
            });
        }
        let Target { texture, buffer, .. } = self.target.as_ref()?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("remote pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        // Read back.
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let submitted = self
            .queue
            .submit(user.into_iter().chain(std::iter::once(encoder.finish())));
        let read = || {
            let slice = buffer.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| drop(tx.send(r)));
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submitted),
                    timeout: Some(GPU_WAIT),
                })
                .ok()?;
            rx.recv().ok()?.ok()?;
            let data = slice.get_mapped_range().ok()?;
            let mut rgba = Vec::with_capacity(unpadded * height as usize);
            for row in data.chunks_exact(padded) {
                rgba.extend_from_slice(&row[..unpadded]);
            }
            drop(data);
            buffer.unmap();
            Some(rgba)
        };
        let rgba = read();
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        // A failed read may leave the buffer mapped: start afresh next time.
        let Some(rgba) = rgba else {
            self.target = None;
            return None;
        };
        Some(Image { width, height, rgba })
    }
}
