//! 2D drawing for the front end and the in-race display: textured
//! rectangles on the game's 640×480 screen, scaled to fit the window with
//! the picture kept at 4:3.

use mm2_formats::tga::Image;
use wgpu::util::DeviceExt;

/// The game's screen size; everything 2D is placed on it.
pub const SCREEN: (f32, f32) = (640.0, 480.0);

/// In the wide layout of the main menu, pieces whose middle is left of or
/// right of these (on the 640×480 screen) are the chain's rollers, logo and
/// the like; between are the panels that open in the middle.
/// 640 and 480, by axis.
const SCREEN_AXIS: [f32; 2] = [SCREEN.0, SCREEN.1];

/// How the menus' layout is spread over a window (`Sprites::layout`).
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// 0: along x (a wide window), 1: along y (a tall one).
    pub axis: usize,
    /// How long the layout is along that axis, in window pixels, and where
    /// it begins.
    pub length: f32,
    pub offset: f32,
    /// Window pixels per unit of the 640×480 screen on the other axis,
    /// which is the scale every piece keeps.
    pub scale: f32,
}

/// Whether a picture is solid all round its edge (see `Texture::tile`).
fn is_tile(image: &Image) -> bool {
    let (w, h) = (image.width as usize, image.height as usize);
    if w < 8 || h < 8 || image.rgba.len() != w * h * 4 {
        return false;
    }
    let solid = |x: usize, y: usize| image.rgba[(y * w + x) * 4 + 3] >= 250;
    let edge = (0..w).flat_map(|x| [(x, 0), (x, h - 1)]).chain((0..h).flat_map(|y| [(0, y), (w - 1, y)]));
    let (mut n, mut ok) = (0, 0);
    for (x, y) in edge {
        n += 1;
        ok += solid(x, y) as usize;
    }
    ok * 100 >= n * 95
}

/// What a queued rectangle is, in the wide layouts.
const KIND_PIECE: u8 = 1;
/// Part of the background art: stretched across the window.
const KIND_ART: u8 = 2;
/// Text: keeps its shape, placed by an anchor (`Sprites::anchor_text`).
const KIND_TEXT: u8 = 3;
/// A panel screen's picture: stretched if wide, else kept and centred.
const KIND_PANEL: u8 = 4;
/// Text in the main menu's layout: keeps its shape.
const KIND_TEXT_KEPT: u8 = 5;
/// The same two, for a picture that is a solid tile (see `Texture::tile`).
const KIND_PIECE_TILE: u8 = 6;
const KIND_PANEL_TILE: u8 = 7;
/// A tile at least this wide (on the 640x480 screen) goes with the art.
const TILE_ART: f32 = 55.0;
/// A panel screen's picture narrower than this (on the 640x480 screen) is
/// an icon, an arrow or a thin line, and keeps its shape.
const SMALL_ART: f32 = 95.0;

/// The widest a window's menu layout is spread (as width over height); a
/// wider window keeps this shape, centred, with the background carried on
/// past it, so that a very wide window does not stretch the art too far.
const MAX_ASPECT: f32 = 2.5;
/// The same for a tall window (height over width): the layout is spread
/// over up to this much of it, centred.
const MAX_TALL: f32 = 2.0;

const SPREAD_LEFT: f32 = 200.0;
const SPREAD_RIGHT: f32 = 440.0;
/// A piece at the sides wider than this (on the 640×480 screen) is part of
/// the art: a roller, not an icon or a word.
const ART_WIDTH: f32 = 70.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureId(usize);

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

struct Texture {
    texture: wgpu::Texture,
    group: wgpu::BindGroup,
    width: u32,
    height: u32,
    /// Its size in 640x480 pixels, where it holds more pixels than it
    /// covers (text drawn at the window's resolution).
    logical: (u32, u32),
    /// Whether the picture is a solid rectangle all round its edge: a
    /// button or icon that carries its own background, which has to go
    /// with the art behind it in a wide layout, or its edge shows.
    tile: bool,
}

pub struct Sprites {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    textures: Vec<Texture>,
    /// This frame's rectangles, by texture.
    /// The third field is whether the quad stands at a place in the 3D
    /// view (a rider's label), with [`widescreen`](Self::widescreen).
    quads: Vec<(TextureId, [Vertex; 4], bool, u8, [f32; 4])>,
    /// The wide layouts, set while a menu screen is drawn: 0 none; 1 the
    /// main menu's (`Sprites::spread_x`); 2 a panel screen's, where the
    /// art is stretched across a wide window and the text keeps its shape.
    pub spread: u8,
    /// Stretch 640×480 over the whole window, as the game's race display
    /// does, rather than keep it 4:3 (the menus).
    pub stretch: bool,
    /// Window pixels per 640x480 pixel, for drawing text at the window's
    /// own resolution.
    pub scale: f32,
    /// With `stretch`, on a window wider than 4:3: keep each piece its
    /// shape and move it to the edge it is nearest, rather than stretch
    /// it (an enhanced-graphics upgrade, not the game's).
    pub widescreen: bool,
    /// Whether the quads drawn now are placed in the 3D view, so only
    /// their place is stretched, not their shape.
    pub in_view: bool,
}

const SHADER: &str = r#"
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>,
           @location(2) color: vec4<f32>) -> Out {
    var out: Out;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    // Tints are the game's own sRGB colours; the target blends in linear.
    let c = color.rgb;
    let lin = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    out.color = vec4<f32>(lin, color.a);
    return out;
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

// Sharp bilinear: a magnified texel stays a solid square, blended with
// its neighbour only across the last screen pixel at its edge, so the
// game's 640x480 art and text keep hard pixels at any window size without
// the uneven rows of plain nearest. Minified images (the hi-res pack)
// sample plainly.
@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(image));
    let texel = in.uv * size;
    let per_pixel = max(fwidth(texel), vec2<f32>(1e-6));
    let edge = max(vec2<f32>(0.5) - 0.5 * per_pixel, vec2<f32>(0.0));
    let d = fract(texel) - 0.5;
    let f = (d - clamp(d, -edge, edge)) / per_pixel + 0.5;
    let uv = select((floor(texel) + f) / size, in.uv, per_pixel >= vec2<f32>(1.0));
    return textureSampleGrad(image, image_sampler, uv, dpdx(in.uv), dpdy(in.uv)) * in.color;
}
"#;

impl Sprites {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprites"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprites"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprites"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Sprites {
            pipeline,
            sampler,
            textures: Vec::new(),
            quads: Vec::new(),
            spread: 0,
            stretch: false,
            scale: 1.0,
            widescreen: false,
            in_view: false,
        }
    }

    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &Image,
    ) -> TextureId {
        self.upload_covering(device, queue, image, (image.width, image.height))
    }

    /// Uploads `image` to cover `logical` 640x480 pixels.
    pub fn upload_covering(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &Image,
        logical: (u32, u32),
    ) -> TextureId {
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("sprite"),
                size: wgpu::Extent3d {
                    width: image.width.max(1),
                    height: image.height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            if image.rgba.is_empty() {
                &[0; 4]
            } else {
                &image.rgba
            },
        );
        let view = texture.create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.textures.push(Texture {
            texture,
            group,
            width: image.width,
            height: image.height,
            logical,
            tile: is_tile(image),
        });
        TextureId(self.textures.len() - 1)
    }

    /// New pixels for a texture of the same size (a movie's next frame).
    pub fn update(&mut self, queue: &wgpu::Queue, id: TextureId, rgba: &[u8]) {
        let t = &self.textures[id.0];
        if rgba.len() != (t.width * t.height * 4) as usize {
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(t.width * 4),
                rows_per_image: Some(t.height),
            },
            wgpu::Extent3d {
                width: t.width,
                height: t.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Drops the quads queued since the last render.
    pub fn discard(&mut self) {
        self.quads.clear();
    }

    pub fn size(&self, id: TextureId) -> (u32, u32) {
        self.textures[id.0].logical
    }

    /// Draws the whole of `id` with its top left at `x`, `y`.
    pub fn image(&mut self, id: TextureId, x: f32, y: f32) {
        let (w, h) = self.size(id);
        self.quad(
            id,
            [0.0, 0.0, 1.0, 1.0],
            [x, y, w as f32, h as f32],
            [1.0; 4],
        );
    }

    /// Draws the part `uv` (left, top, right, bottom, 0 to 1) of `id` into
    /// `rect` (x, y, width, height on the 640×480 screen), tinted.
    pub fn quad(&mut self, id: TextureId, uv: [f32; 4], rect: [f32; 4], color: [f32; 4]) {
        let [x, y, w, h] = rect;
        let v = |px: f32, py: f32, u: f32, t: f32| Vertex {
            position: [px, py],
            uv: [u, t],
            color,
        };
        self.quads.push((
            id,
            [
                v(x, y, uv[0], uv[1]),
                v(x + w, y, uv[2], uv[1]),
                v(x + w, y + h, uv[2], uv[3]),
                v(x, y + h, uv[0], uv[3]),
            ],
            self.in_view,
            self.kind_for(id),
            [0.0; 4],
        ));
    }

    /// Draws the whole of `id` stretched over four corners (clockwise from
    /// the top left), for shapes that are not upright rectangles.
    pub fn corners(&mut self, id: TextureId, corners: [[f32; 2]; 4], color: [f32; 4]) {
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        self.quads.push((
            id,
            std::array::from_fn(|i| Vertex {
                position: corners[i],
                uv: uv[i],
                color,
            }),
            self.in_view,
            self.kind_for(id),
            [0.0; 4],
        ));
    }

    /// Draws this frame's rectangles over `target` and clears the list.
    /// With `clear`, the window is first filled with black.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        (width, height): (u32, u32),
        clear: bool,
    ) {
        // Fit 640×480 into the window, centred, or stretch it.
        let (sx, sy, ox, oy) = if self.stretch {
            (width as f32 / SCREEN.0, height as f32 / SCREEN.1, 0.0, 0.0)
        } else {
            Self::menu_fit((width, height))
        };
        let ndc = |p: [f32; 2]| {
            [
                (ox + p[0] * sx) / width as f32 * 2.0 - 1.0,
                1.0 - (oy + p[1] * sy) / height as f32 * 2.0,
            ]
        };
        let mut vertices = Vec::with_capacity(self.quads.len() * 4);
        let mut indices: Vec<u32> = Vec::with_capacity(self.quads.len() * 6);
        let mut batches: Vec<(TextureId, std::ops::Range<u32>)> = Vec::new();
        // Widescreen: one scale for both ways, and each piece moved to the
        // edge it is nearest, by the third of the screen its middle is in.
        let wide = (self.stretch && self.widescreen && width * 3 > height * 4)
            .then(|| (height as f32 / SCREEN.1, width as f32 / SCREEN.0));
        let layout = Self::layout((width, height));
        for (id, quad, in_view, kind, anchor) in &self.quads {
            let base = vertices.len() as u32;
            if let (Some(lay), true) = (layout, *kind > 0) {
                // The axis the layout is spread along (x in a wide window, y
                // in a tall one), and the other, where every piece has the
                // scale of the window.
                let (ax, cross) = (lay.axis, 1 - lay.axis);
                let along = |v: &Vertex| v.position[ax];
                let middle = quad.iter().map(along).sum::<f32>() / 4.0;
                let (left, right) = quad.iter().map(along).fold((f32::MAX, f32::MIN), |(l, r), c| (l.min(c), r.max(c)));
                let extent = right - left;
                // Where the piece is across the spread axis, and how wide: the
                // chain's rollers are the wide pieces at the sides.
                let across_mid = quad.iter().map(|v| v.position[cross]).sum::<f32>() / 4.0;
                // In the main menu the pieces at the sides are the chain's
                // rollers and what sits on them: all art, whatever their
                // size, so they stretch with the background's own.
                // (Left and right are by x: along the axis in a wide window,
                // across it in a tall one.)
                let x_mid = if ax == 0 { middle } else { across_mid };
                let side = x_mid < SPREAD_LEFT || x_mid > SPREAD_RIGHT;
                let tile = matches!(*kind, KIND_PIECE_TILE | KIND_PANEL_TILE) && extent >= TILE_ART;
                let stretch = |c: f32| lay.offset + c * lay.length / SCREEN_AXIS[ax];
                // Where a piece that keeps its shape stands: its middle at
                // `at` (window pixels).
                let kept = |c: f32, at: f32| at + (c - middle) * lay.scale;
                for v in quad {
                    let c0 = v.position[ax];
                    let c = match *kind {
                        KIND_ART => stretch(c0),
                        KIND_TEXT => stretch(anchor[2 * ax]) - anchor[2 * ax + 1] * extent * lay.scale + (c0 - left) * lay.scale,
                        KIND_PANEL | KIND_PANEL_TILE if extent > SMALL_ART || tile => stretch(c0),
                        KIND_PANEL | KIND_PANEL_TILE => kept(c0, stretch(middle)),
                        // Text in the main menu keeps its shape where the
                        // layout puts it.
                        KIND_TEXT_KEPT if ax == 0 => kept(c0, lay.offset + Self::spread_x(middle, lay.length, lay.scale)),
                        KIND_TEXT_KEPT => kept(c0, stretch(middle)),
                        // The main menu: a piece over the whole screen, the
                        // dim behind a message box, goes with the
                        // background; so do the rollers at the sides; the
                        // rest, the panels that open in the middle with all
                        // that is on them (tiles included), keep their shape
                        // together.
                        _ if extent >= SCREEN_AXIS[ax] * 0.9 => stretch(c0),
                        _ if side && ax == 0 => stretch(c0),
                        _ if ax == 0 => kept(c0, lay.offset + Self::spread_x(middle, lay.length, lay.scale)),
                        // In a tall window it goes the same way, by where
                        // the piece is across.
                        _ if side || extent > SMALL_ART => stretch(c0),
                        _ => kept(c0, stretch(middle)),
                    };
                    let mut at = [0.0; 2];
                    at[ax] = c;
                    at[cross] = v.position[cross] * lay.scale;
                    vertices.push(Vertex {
                        position: [at[0] / width as f32 * 2.0 - 1.0, 1.0 - at[1] / height as f32 * 2.0],
                        ..*v
                    });
                }
                let start = indices.len() as u32;
                indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
                match batches.last_mut() {
                    Some((last, range)) if last == id => range.end = indices.len() as u32,
                    _ => batches.push((*id, start..indices.len() as u32)),
                }
                continue;
            }
            let shift = wide.map(|(s, stretched)| {
                let middle = quad.iter().map(|v| v.position[0]).sum::<f32>() / 4.0;
                let anchor = if *in_view {
                    // Stays over its rider: its corner at its stretched place.
                    quad[0].position[0]
                } else if middle < SCREEN.0 / 3.0 {
                    0.0
                } else if middle > SCREEN.0 * 2.0 / 3.0 {
                    SCREEN.0
                } else {
                    SCREEN.0 / 2.0
                };
                (anchor, anchor * stretched, s)
            });
            for v in quad {
                let position = match shift {
                    Some((anchor, to, s)) => [
                        (to + (v.position[0] - anchor) * s) / width as f32 * 2.0 - 1.0,
                        1.0 - v.position[1] * s / height as f32 * 2.0,
                    ],
                    None => ndc(v.position),
                };
                vertices.push(Vertex { position, ..*v });
            }
            let start = indices.len() as u32;
            indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
            match batches.last_mut() {
                Some((last, range)) if last == id => range.end = indices.len() as u32,
                _ => batches.push((*id, start..indices.len() as u32)),
            }
        }
        self.quads.clear();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sprites"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if indices.is_empty() {
            return;
        }
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sprite vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sprite indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
        for (id, range) in batches {
            pass.set_bind_group(0, &self.textures[id.0].group, &[]);
            pass.draw_indexed(range, 0, 0..1);
        }
    }

    /// Where a window position falls on the 640×480 screen.
    pub fn to_screen((width, height): (u32, u32), x: f64, y: f64) -> (f32, f32) {
        Self::to_screen_as((width, height), x, y, false)
    }

    /// The same, for sprites drawn with [`stretch`](Self::stretch) on (the
    /// race's HUD and the replay player), which fill the window.
    pub fn to_screen_as((width, height): (u32, u32), x: f64, y: f64, stretch: bool) -> (f32, f32) {
        if stretch {
            return (
                x as f32 * SCREEN.0 / width.max(1) as f32,
                y as f32 * SCREEN.1 / height.max(1) as f32,
            );
        }
        let (sx, sy, ox, oy) = Self::menu_fit((width, height));
        ((x as f32 - ox) / sx, (y as f32 - oy) / sy)
    }

    /// How the menus' 640×480 fits a window: window pixels per unit across
    /// and down (the same), and the left and top edges: the largest 4:3
    /// that fits, centred.
    pub fn menu_fit((width, height): (u32, u32)) -> (f32, f32, f32, f32) {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let scale = (w / SCREEN.0).min(h / SCREEN.1);
        (scale, scale, (w - SCREEN.0 * scale) / 2.0, (h - SCREEN.1 * scale) / 2.0)
    }

    /// A menu's background picture, which fills the 640×480 screen, carried
    /// on past its edges by repeating its outermost pixels, so that the
    /// window beyond the 4:3 is filled and not left black. The pictures'
    /// edges are plain dark bands, which this continues.
    pub fn backdrop(&mut self, id: TextureId) {
        // Far enough for any window up to 4:1 either way.
        self.quad(id, [-1.0, -1.0, 2.0, 2.0], [-SCREEN.0, -SCREEN.1, SCREEN.0 * 3.0, SCREEN.1 * 3.0], [1.0; 4]);
        // In the wide layout this picture is the one that is stretched.
        if let Some(q) = self.quads.last_mut().filter(|_| self.spread > 0) {
            q.3 = KIND_ART;
        }
    }

    /// A rectangle that goes with the background picture, stretched across
    /// a wide window with it (in the main menu's and the full-screen
    /// screens' wide layout; elsewhere an ordinary one).
    pub fn stretched(&mut self, id: TextureId, uv: [f32; 4], rect: [f32; 4]) {
        self.stretched_tinted(id, uv, rect, [1.0; 4]);
    }

    /// `stretched`, tinted.
    pub fn stretched_tinted(&mut self, id: TextureId, uv: [f32; 4], rect: [f32; 4], color: [f32; 4]) {
        self.quad(id, uv, rect, color);
        if let Some(q) = self.quads.last_mut().filter(|_| self.spread > 0) {
            q.3 = KIND_ART;
        }
    }

    /// What a rectangle of `id` queued now is, in the wide layout drawing.
    fn kind_for(&self, id: TextureId) -> u8 {
        let tile = self.textures[id.0].tile;
        match (self.spread, tile) {
            (1, false) => KIND_PIECE,
            (1, true) => KIND_PIECE_TILE,
            (2, false) => KIND_PANEL,
            (2, true) => KIND_PANEL_TILE,
            _ => 0,
        }
    }

    /// Says where the last `count` rectangles, a piece of text and its
    /// shadow, stand in a panel screen's wide layout: `at` (on the 640x480
    /// screen) is where the text's left (`along` 0), middle (0.5) or right
    /// (1) edge goes, and the text keeps its own shape.
    pub fn anchor_text(&mut self, count: usize, across: (f32, f32), down: (f32, f32)) {
        if self.spread == 0 {
            return;
        }
        let from = self.quads.len().saturating_sub(count);
        for q in &mut self.quads[from..] {
            q.3 = if self.spread == 2 { KIND_TEXT } else { KIND_TEXT_KEPT };
            q.4 = [across.0, across.1, down.0, down.1];
        }
    }

    /// How the menus' wide (or tall) layout is spread over a window: along
    /// x in a window wider than 4:3, along y in one taller, none in between.
    pub fn layout((width, height): (u32, u32)) -> Option<Layout> {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        if w * 3.0 > h * 4.0 {
            let length = w.min(h * MAX_ASPECT);
            Some(Layout { axis: 0, length, offset: (w - length) / 2.0, scale: h / SCREEN.1 })
        } else if h * 4.0 > w * 3.0 {
            let length = h.min(w * MAX_TALL);
            Some(Layout { axis: 1, length, offset: (h - length) / 2.0, scale: w / SCREEN.0 })
        } else {
            None
        }
    }

    /// The main menu's wide layout, where the window is `width` across: the
    /// background picture is stretched over the whole window, while every
    /// other piece (icons, text, panels) keeps its shape, at the scale of
    /// the height. A piece in the left or right of the screen stands where
    /// the stretched picture puts its middle; the pieces between, the
    /// panels that open in the middle, stand together about the window's
    /// middle. Returns, for a piece whose middle is at `centre` on the
    /// 640×480 screen, its window x of that middle.
    fn spread_x(centre: f32, width: f32, scale: f32) -> f32 {
        if centre < SPREAD_LEFT || centre > SPREAD_RIGHT {
            centre * width / SCREEN.0
        } else {
            width / 2.0 + (centre - SCREEN.0 / 2.0) * scale
        }
    }

    /// Where a window position falls on the 640×480 screen in the wide
    /// layout (for the pointer): the stretched way at the sides, about the
    /// middle between. A piece is hit a little beyond its drawn width.
    pub fn to_screen_spread(size: (u32, u32), x: f64, y: f64, mode: u8) -> (f32, f32) {
        let Some(lay) = Self::layout(size) else {
            return Self::to_screen_as(size, x, y, false);
        };
        let (x, y) = (x as f32, y as f32);
        let (a, other) = if lay.axis == 0 { (x, y) } else { (y, x) };
        let unit_cross = other / lay.scale;
        let a = a - lay.offset;
        let unit = if lay.axis == 0 && mode == 1 {
            // The main menu: the stretched way at the sides, about the
            // middle between.
            let side_edge = SPREAD_LEFT * lay.length / SCREEN.0;
            let middle_edge = lay.length / 2.0 + (SPREAD_LEFT - SCREEN.0 / 2.0) * lay.scale;
            let split = (side_edge + middle_edge) / 2.0;
            if a < split || a > lay.length - split {
                a * SCREEN.0 / lay.length
            } else {
                (a - lay.length / 2.0) / lay.scale + SCREEN.0 / 2.0
            }
        } else {
            a * SCREEN_AXIS[lay.axis] / lay.length
        };
        if lay.axis == 0 { (unit, unit_cross) } else { (unit_cross, unit) }
    }}
