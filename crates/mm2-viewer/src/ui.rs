//! The front end's building blocks: the game's dialogs from `UI/*.DAT`
//! (layout in `.dtm`, art as `.tga`, sounds as `.wav`), shared art and
//! sounds from `UI/GLOBAL.DAT`, menu text from `UILANG.DLL`, and text
//! drawn in Arial as the game's GDI did.
//!
//! [`Ui`] loads and caches all of that; [`Dlg`] is one dialog on screen
//! with the state of its controls.

use crate::audio::Audio;
use crate::sprites::{Sprites, TextureId};
use mm2_formats::dtm::{self, Control, Dialog, Kind};
use mm2_formats::{rs2, tga, wav};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Makes the see-through magenta clear, and gives the pixels it leaves the
/// colour of their neighbours for two pixels out, which is as far as a
/// smoothed edge reaches. (`hires::key_rgba` fills to the far end of a big
/// clear area, which is slow for a picture that is mostly clear.)
fn key_and_bleed(rgba: &mut [u8], width: usize, height: usize) {
    let mut keyed = false;
    for px in rgba.chunks_exact_mut(4) {
        if px[..3] == [255, 0, 255] {
            px.copy_from_slice(&[0; 4]);
            keyed = true;
        }
    }
    if !keyed {
        return;
    }
    let mut filled: Vec<bool> = rgba.chunks_exact(4).map(|p| p[3] != 0).collect();
    for _ in 0..2 {
        let mut next = filled.clone();
        for y in 0..height {
            for x in 0..width {
                let i = y * width + x;
                if filled[i] {
                    continue;
                }
                let (mut sum, mut n) = ([0u32; 3], 0u32);
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let j = ny as usize * width + nx as usize;
                    if filled[j] {
                        for c in 0..3 {
                            sum[c] += rgba[j * 4 + c] as u32;
                        }
                        n += 1;
                    }
                }
                if n > 0 {
                    for c in 0..3 {
                        rgba[i * 4 + c] = (sum[c] / n) as u8;
                    }
                    next[i] = true;
                }
            }
        }
        filled = next;
    }
}

/// What drawing needs from the window.
pub struct Gfx<'a> {
    pub sprites: &'a mut Sprites,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
}

/// A `UI/*.DAT` archive and its dialog.
struct Pack {
    bytes: Vec<u8>,
    dialog: Dialog,
}

impl Pack {
    fn load(path: &Path) -> Option<Pack> {
        let bytes = crate::platform::fs::read(path)
            .map_err(|e| eprintln!("{}: {e}", path.display()))
            .ok()?;
        let dialog = {
            let ar = rs2::Archive::parse(&bytes).ok()?;
            let dtm = ar
                .entries()
                .iter()
                .find(|e| e.name.to_ascii_lowercase().ends_with(".dtm"))?;
            dtm::parse(ar.data(dtm))
        };
        Some(Pack { bytes, dialog })
    }

    /// A file by its archive name, or by its last path part.
    fn file(&self, name: &str) -> Option<&[u8]> {
        let ar = rs2::Archive::parse(&self.bytes).ok()?;
        if let Some(data) = ar.get(name) {
            return Some(data);
        }
        let base = name.rsplit(['\\', '/']).next()?;
        ar.entries()
            .iter()
            .find(|e| {
                e.name
                    .rsplit('\\')
                    .next()
                    .is_some_and(|n| n.eq_ignore_ascii_case(base))
            })
            .map(|e| ar.data(e))
    }
}

/// Text in one size and weight, as GDI drew it: a system Arial (or a
/// metric twin), else the game's own small bitmap font.
struct Fonts {
    regular: Option<ab_glyph::FontVec>,
    bold: Option<ab_glyph::FontVec>,
    bold_italic: Option<ab_glyph::FontVec>,
    italic: Option<ab_glyph::FontVec>,
}

/// Liberation Sans, which has Arial's letter widths, built into the browser
/// build (regular, bold, bold italic, italic) so the text does not depend on
/// the page having handed it over before the menus start. Empty on desktop.
#[cfg(target_arch = "wasm32")]
const BUNDLED: [&[u8]; 4] = [
    include_bytes!("../fonts/LiberationSans-Regular.ttf"),
    include_bytes!("../fonts/LiberationSans-Bold.ttf"),
    include_bytes!("../fonts/LiberationSans-BoldItalic.ttf"),
    include_bytes!("../fonts/LiberationSans-Italic.ttf"),
];
#[cfg(not(target_arch = "wasm32"))]
const BUNDLED: [&[u8]; 4] = [&[]; 4];

impl Fonts {
    fn load() -> Self {
        let find = |face: usize, names: &[&str]| {
            let found = names.iter().find_map(|p| {
                let bytes = crate::platform::fs::read(p).ok()?;
                ab_glyph::FontVec::try_from_vec(bytes).ok()
            });
            if found.is_some() || BUNDLED[face].is_empty() {
                return found;
            }
            #[cfg(target_arch = "wasm32")]
            web_sys::console::warn_1(
                &format!("font {face} not in the file system; using the built-in Liberation Sans").into(),
            );
            ab_glyph::FontVec::try_from_vec(BUNDLED[face].to_vec()).ok()
        };
        Fonts {
            regular: find(0, &[
                "/System/Library/Fonts/Supplemental/Arial.ttf",
                "/Library/Fonts/Arial.ttf",
                "C:\\Windows\\Fonts\\arial.ttf",
                "/usr/share/fonts/truetype/msttcorefonts/Arial.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
                "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
            ]),
            bold: find(1, &[
                "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
                "/Library/Fonts/Arial Bold.ttf",
                "C:\\Windows\\Fonts\\arialbd.ttf",
                "/usr/share/fonts/truetype/msttcorefonts/Arial_Bold.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
                "/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf",
            ]),
            bold_italic: find(2, &[
                "/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf",
                "/Library/Fonts/Arial Bold Italic.ttf",
                "C:\\Windows\\Fonts\\arialbi.ttf",
                "/usr/share/fonts/truetype/msttcorefonts/Arial_Bold_Italic.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-BoldItalic.ttf",
                "/usr/share/fonts/liberation-sans/LiberationSans-BoldItalic.ttf",
            ]),
            italic: find(3, &[
                "/System/Library/Fonts/Supplemental/Arial Italic.ttf",
                "/Library/Fonts/Arial Italic.ttf",
                "C:\\Windows\\Fonts\\ariali.ttf",
                "/usr/share/fonts/truetype/msttcorefonts/Arial_Italic.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Italic.ttf",
                "/usr/share/fonts/liberation-sans/LiberationSans-Italic.ttf",
            ]),
        }
    }

    /// `text` rendered white on clear, `em` whole pixels to the em as GDI
    /// sizes a TrueType face; the image is the cell, ascent to descent.
    /// `scale` window pixels to each of the game's: the text keeps its
    /// 640x480 layout (returned as the logical size) and is drawn smoothly
    /// at the finer resolution; at 1 it is GDI's unsmoothed pixels.
    fn render(
        &self,
        text: &str,
        em: f32,
        bold: bool,
        italic: bool,
        cut: f32,
        scale: f32,
    ) -> Option<(tga::Image, (u32, u32))> {
        use ab_glyph::{Font, ScaleFont};
        let upright = if bold {
            self.bold.as_ref().or(self.regular.as_ref())
        } else {
            self.regular.as_ref().or(self.bold.as_ref())
        };
        let slanted = if bold {
            &self.bold_italic
        } else {
            &self.italic
        };
        let font = if italic {
            slanted.as_ref().or(upright)
        } else {
            upright
        }?;
        let px = em * font.height_unscaled() / font.units_per_em().unwrap_or(2048.0);
        let scaled = font.as_scaled(px);
        // GDI keeps the baseline on a whole pixel.
        let ascent = scaled.ascent().round();
        let mut x = 0.0f32;
        let mut placed = Vec::new();
        let mut last = None;
        for ch in text.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(prev) = last {
                x += scaled.kern(prev, id);
            }
            placed.push((id, x));
            x += scaled.h_advance(id);
            last = Some(id);
        }
        let logical = (
            x.ceil().max(1.0) as u32,
            (ascent - scaled.descent().round()).max(1.0) as u32,
        );
        let smooth = scale > 1.01;
        let s = if smooth { scale } else { 1.0 };
        let width = (logical.0 as f32 * s).ceil() as u32;
        let height = (logical.1 as f32 * s).ceil() as u32;
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let big = ab_glyph::PxScale::from(px * s);
        for (id, x) in placed {
            let g = id.with_scale_and_position(big, ab_glyph::point(x * s, ascent * s));
            let Some(outline) = font.outline_glyph(g) else {
                continue;
            };
            let b = outline.px_bounds();
            outline.draw(|gx, gy, c| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                    return;
                }
                let i = ((py as u32 * width + px as u32) * 4) as usize;
                rgba[i..i + 3].copy_from_slice(&[255, 255, 255]);
                let a = if smooth {
                    (c.min(1.0) * 255.0).round() as u8
                } else if c >= cut {
                    // GDI drew the menus' text without smoothing.
                    255
                } else {
                    0
                };
                rgba[i + 3] = rgba[i + 3].max(a);
            });
        }
        Some((
            tga::Image {
                width,
                height,
                rgba,
            },
            logical,
        ))
    }
}

/// Everything the front end loads, cached.
pub struct Ui {
    dir: PathBuf,
    global: Option<Pack>,
    packs: HashMap<String, Option<Pack>>,
    images: HashMap<(String, String), Option<TextureId>>,
    sounds: HashMap<String, Option<Arc<wav::Sound>>>,
    texts: HashMap<(String, u32, bool, bool), Option<TextureId>>,
    /// The window scale the cached texts were drawn for.
    text_scale: f32,
    /// Tooltip texts, by tenths of their em.
    tips: HashMap<(String, u32), Option<TextureId>>,
    /// `UILANG.DLL`, the menus' own text.
    strings: HashMap<u32, String>,
    /// `LANG.DLL`, text the game's code sets.
    lang: HashMap<u32, String>,
    fonts: Fonts,
    /// A white pixel, for filled rectangles.
    white: Option<TextureId>,
    /// The menus' sounds' volume (the Sound page's), 0 to 1.
    pub volume: f32,
    /// The mixer, for the movies.
    pub audio: Option<crate::audio::Audio>,
}

impl Ui {
    /// The front end of the game in `game` (the folder with `UI` and
    /// `UILANG.DLL`).
    pub fn new(game: &Path) -> Self {
        let dir = game.join("UI");
        let strings = |name: &str| {
            crate::platform::fs::read(game.join(name))
                .ok()
                .and_then(|b| mm2_formats::pe::strings(&b).ok())
                .unwrap_or_default()
        };
        Ui {
            global: Pack::load(&dir.join("GLOBAL.DAT")),
            dir,
            packs: HashMap::new(),
            images: HashMap::new(),
            sounds: HashMap::new(),
            texts: HashMap::new(),
            text_scale: 1.0,
            tips: HashMap::new(),
            strings: strings("UILANG.DLL"),
            lang: strings("LANG.DLL"),
            fonts: Fonts::load(),
            white: None,
            volume: 1.0,
            audio: None,
        }
    }

    /// The game folder.
    pub fn game(&self) -> &Path {
        self.dir.parent().unwrap_or(&self.dir)
    }

    /// A string the game's code shows, from `LANG.DLL`.
    pub fn lang(&self, id: u32) -> String {
        self.lang.get(&id).cloned().unwrap_or_default()
    }

    /// A white pixel's texture, made on first use.
    fn white(&mut self, gfx: &mut Gfx) -> TextureId {
        *self.white.get_or_insert_with(|| {
            let img = tga::Image {
                width: 1,
                height: 1,
                rgba: vec![255; 4],
            };
            gfx.sprites.upload(gfx.device, gfx.queue, &img)
        })
    }

    /// Fills `rect` with `color`.
    pub fn fill(&mut self, gfx: &mut Gfx, rect: [f32; 4], color: [f32; 4]) {
        let white = self.white(gfx);
        gfx.sprites.quad(white, [0.0, 0.0, 1.0, 1.0], rect, color);
    }

    /// A line `width` pixels wide from `a` to `b`.
    pub fn line(&mut self, gfx: &mut Gfx, a: [f32; 2], b: [f32; 2], width: f32, color: [f32; 4]) {
        let white = self.white(gfx);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt().max(1e-3);
        let (nx, ny) = (-dy / len * width / 2.0, dx / len * width / 2.0);
        gfx.sprites.corners(
            white,
            [
                [a[0] + nx, a[1] + ny],
                [b[0] + nx, b[1] + ny],
                [b[0] - nx, b[1] - ny],
                [a[0] - nx, a[1] - ny],
            ],
            color,
        );
    }

    /// One of the race's overlay pictures in `RES/GLOBAL.RES` (they carry
    /// their own alpha).
    pub fn overlay(&mut self, gfx: &mut Gfx, name: &str) -> Option<TextureId> {
        let path = self.game().join("RES").join("GLOBAL.RES");
        self.picture(gfx, &format!("GLOBAL.RES#{name}"), || {
            let bytes = crate::read_archive(&path).ok()?;
            let ar = rs2::Archive::parse(&bytes).ok()?;
            let e = ar
                .entries()
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(name))?;
            mm2_formats::texture::parse_overlay(ar.data(e))
                .map_err(|e| eprintln!("{name}: {e}"))
                .ok()
        })
    }

    /// An image from an archive that is not a dialog's (a track's preview),
    /// cached under `key`.
    pub fn picture(
        &mut self,
        gfx: &mut Gfx,
        key: &str,
        load: impl FnOnce() -> Option<tga::Image>,
    ) -> Option<TextureId> {
        let key = ("".to_string(), key.to_ascii_lowercase());
        if let Some(t) = self.images.get(&key) {
            return *t;
        }
        let tex = load().map(|img| gfx.sprites.upload(gfx.device, gfx.queue, &img));
        self.images.insert(key, tex);
        tex
    }

    fn pack(&mut self, name: &str) -> Option<&Pack> {
        let key = name.to_ascii_uppercase();
        if !self.packs.contains_key(&key) {
            let pack = Pack::load(&self.dir.join(format!("{key}.DAT")));
            self.packs.insert(key.clone(), pack);
        }
        self.packs[&key].as_ref()
    }

    /// The dialog in `UI/<name>.DAT`.
    pub fn dialog(&mut self, name: &str) -> Option<Dialog> {
        self.pack(name).map(|p| p.dialog.clone())
    }

    /// A menu string from `UILANG.DLL`.
    pub fn string(&self, id: u32) -> Option<&str> {
        self.strings.get(&id).map(String::as_str)
    }

    /// Finds an image's file data: the dialog's own `[Set_Anim]` name, then
    /// GLOBAL's, then a file of that name in either archive.
    fn image_bytes(&mut self, pack: &str, name: &str) -> Option<Vec<u8>> {
        self.pack(pack);
        let own = self.packs[&pack.to_ascii_uppercase()].as_ref();
        let global = self.global.as_ref();
        let file = own
            .and_then(|p| p.dialog.anim(name))
            .or_else(|| global?.dialog.anim(name))
            .and_then(|a| a.frames.first().cloned())
            .unwrap_or_else(|| format!("{name}.tga"));
        own.and_then(|p| p.file(&file))
            .or_else(|| global?.file(&file))
            .map(<[u8]>::to_vec)
    }

    /// An image as a texture, magenta made clear.
    pub fn image(&mut self, gfx: &mut Gfx, pack: &str, name: &str) -> Option<TextureId> {
        let key = (pack.to_ascii_uppercase(), name.to_ascii_lowercase());
        if let Some(t) = self.images.get(&key) {
            return *t;
        }
        let tex = self
            .image_bytes(pack, name)
            .and_then(|b| tga::parse(&b).map_err(|e| eprintln!("{name}: {e}")).ok())
            .map(|mut img| {
                // The see-through magenta goes clear, and the pixels it
                // leaves take the colour of their neighbours: left magenta,
                // the picture's edge shows a pink line wherever it is
                // smoothed, which a wide layout does everywhere.
                let (w, h) = (img.width as usize, img.height as usize);
                if img.rgba.len() == w * h * 4 {
                    key_and_bleed(&mut img.rgba, w, h);
                }
                gfx.sprites.upload(gfx.device, gfx.queue, &img)
            });
        if tex.is_none() {
            eprintln!("no image {name} for {pack}");
        }
        self.images.insert(key, tex);
        tex
    }

    /// A sound by name (`Beep13`), from the dialog's archive or GLOBAL's.
    pub fn sound(&mut self, pack: &str, name: &str) -> Option<Arc<wav::Sound>> {
        let key = name.to_ascii_lowercase();
        if let Some(s) = self.sounds.get(&key) {
            return s.clone();
        }
        let file = format!("{name}.wav");
        self.pack(pack);
        let own = self.packs[&pack.to_ascii_uppercase()].as_ref();
        let sound = own
            .and_then(|p| p.file(&file))
            .or_else(|| self.global.as_ref()?.file(&file))
            .and_then(|b| wav::parse(b).ok())
            .map(Arc::new);
        self.sounds.insert(key, sound.clone());
        sound
    }

    /// `text` as a white texture. Captures of the original put a
    /// `FontHeight` of 14.25 and of 15 at 12 pixels to the em, and 15.75
    /// at 13: about 0.83 of it, to whole pixels.
    pub fn text(&mut self, gfx: &mut Gfx, text: &str, pt: f32, bold: bool) -> Option<TextureId> {
        self.styled_text(gfx, text, em_of(pt) * CELL, bold, false)
    }

    /// `text` as `text` makes it, in italics when `italic`.
    pub fn text_in(
        &mut self,
        gfx: &mut Gfx,
        text: &str,
        pt: f32,
        bold: bool,
        italic: bool,
    ) -> Option<TextureId> {
        self.styled_text(gfx, text, em_of(pt) * CELL, bold, italic)
    }

    /// `text` in a font made `cell` pixels high, as `CreateFont` with a
    /// positive height makes it: Arial's cell is [`CELL`] em.
    pub fn cell_text(
        &mut self,
        gfx: &mut Gfx,
        text: &str,
        cell: f32,
        bold: bool,
    ) -> Option<TextureId> {
        self.styled_text(gfx, text, cell, bold, false)
    }

    /// Forgets texts drawn for another window size.
    fn match_scale(&mut self, gfx: &Gfx) {
        if gfx.sprites.scale != self.text_scale {
            self.text_scale = gfx.sprites.scale;
            self.texts.clear();
            self.tips.clear();
        }
    }

    /// A tooltip's text: GDI's hinted small Arial measures like a
    /// fractional em, and keeps strokes that half-pixel coverage would drop.
    pub fn tip_text(&mut self, gfx: &mut Gfx, text: &str, em: f32) -> Option<TextureId> {
        self.match_scale(gfx);
        let key = (text.to_string(), (em * 10.0).round() as u32);
        if let Some(t) = self.tips.get(&key) {
            return *t;
        }
        let tex = self
            .fonts
            .render(text, em, false, false, 0.3, self.text_scale)
            .map(|(img, size)| {
                gfx.sprites
                    .upload_covering(gfx.device, gfx.queue, &img, size)
            });
        self.tips.insert(key, tex);
        tex
    }

    fn styled_text(
        &mut self,
        gfx: &mut Gfx,
        text: &str,
        cell: f32,
        bold: bool,
        italic: bool,
    ) -> Option<TextureId> {
        self.match_scale(gfx);
        let px = (cell / CELL).round().max(1.0) as u32;
        let key = (text.to_string(), px, bold, italic);
        if let Some(t) = self.texts.get(&key) {
            return *t;
        }
        let tex = self
            .fonts
            .render(text, px as f32, bold, italic, 0.5, self.text_scale)
            .map(|(img, size)| {
                gfx.sprites
                    .upload_covering(gfx.device, gfx.queue, &img, size)
            });
        self.texts.insert(key, tex);
        tex
    }

    pub fn play(&mut self, audio: &Audio, pack: &str, name: &str) {
        if self.volume <= 0.0 {
            return;
        }
        if let Some(s) = self.sound(pack, name) {
            audio.play(&s, self.volume, false);
        }
    }

    /// How wide `text` is in pixels.
    pub fn text_width(&mut self, gfx: &mut Gfx, text: &str, pt: f32, bold: bool) -> f32 {
        self.text(gfx, text, pt, bold)
            .map_or(0.0, |t| gfx.sprites.size(t).0 as f32)
    }
}

/// The box behind a picked line, for lists that name no colours: the
/// dialogs' light blue (0x7ab9fe).
const SELECT_BOX: i32 = 16693626;

/// How long the pointer rests on a control before its tip shows.
const TIP_DELAY: f32 = 1.0;
/// The tip's em size, fitted to the widths of three captured tips.
const TIP_EM: f32 = 9.5;

/// A control's tooltip. The page tabs, radio buttons along the top edge,
/// never showed theirs in the original; other radio buttons do.
fn tip_text(c: &Control) -> Option<String> {
    if c.kind == Kind::RadioButton && c.rect[1] == 0 {
        return None;
    }
    c.get("ToolTipText")
        .filter(|t| !t.is_empty())
        .map(|t| t.replace("&&", "&"))
}

/// A greyed button's text (0x9c9c9c).
const GREYED: i32 = 0x9c9c9c;

/// Arial's cell (ascent and descent) in ems.
const CELL: f32 = 1.117;

/// The pixels to the em of a dialog's `FontHeight`.
fn em_of(pt: f32) -> f32 {
    (pt * 0.83).round().max(1.0)
}

/// A VB colour (0x00BBGGRR) as RGBA.
pub fn color(bgr: i32) -> [f32; 4] {
    let c = bgr as u32;
    [
        (c & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        ((c >> 16) & 0xff) as f32 / 255.0,
        1.0,
    ]
}

/// A pull-down's opening or closing frames.
#[derive(Debug, Clone, Copy)]
struct Fx {
    start: f32,
    opening: bool,
}

/// Frame time of a pull-down (our pick: 15 frames in half a second).
const FX_FRAME: f32 = 1.0 / 30.0;

/// The live state of one control.
#[derive(Debug, Clone)]
pub struct Ctl {
    pub visible: bool,
    pub enabled: bool,
    /// Radio buttons and check boxes.
    pub on: bool,
    pub text: Option<String>,
    /// A drop-down list's or list box's lines, and the one chosen.
    pub items: Vec<String>,
    pub selected: usize,
    /// The first line a drop-down's open list or a list box shows.
    pub scroll: usize,
    fx: Option<Fx>,
    /// Slider state: the thumb's offset in pixels from the start of the
    /// track (a float at `0x1EC` in the game), how many notches it snaps
    /// to (`TickNum` minus one, or 0 for free movement), the full-scale
    /// value (`Scale`, default 100) and whether it runs backwards.
    pub at: f32,
    pub steps: u32,
    pub range: u32,
    pub reversed: bool,
    /// The thumb's size, once its picture is known.
    thumb: (f32, f32),
    /// The longest text an edit box takes (`MaxLen`), and the letters it
    /// takes besides letters and digits (any when empty; `0x474ed0`).
    pub max_len: usize,
    pub allowed: String,
    /// A slider's value waiting for its thumb's size.
    pending: Option<(u32, u32)>,
    /// A list box's picked line, and whether the pointer may pick one.
    pub mark: Option<usize>,
    pub selectable: bool,
    /// The text breaks into lines at the control's width (the game's
    /// `0x1e8`, set for prompts).
    pub wrap: bool,
}

impl Ctl {
    /// The chosen line, if the list has one.
    pub fn item(&self) -> Option<&str> {
        self.items.get(self.selected).map(String::as_str)
    }
}

/// Something a dialog asks of the screen around it.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Clicked(String),
    /// A line was picked from a drop-down list or a list box.
    Selected(String, usize),
    Sound(String),
    /// A slider moved (the game's event 3), or an edit box's text changed.
    Changed(String),
    /// A slider was let go (event 16).
    Released(String),
}

/// A drop-down list that is open: where its lines are on the screen.
#[derive(Debug, Clone, Copy)]
struct List {
    control: usize,
    /// x, y, width of the lines; their height is [`LINE`].
    lines: [f32; 3],
    rows: usize,
    hover: Option<usize>,
}

/// The height of a line in an open drop-down list (the dialogs' 15 pixel
/// font).
const LINE: f32 = 15.0;

/// One dialog on screen.
pub struct Dlg {
    pub pack: String,
    pub dialog: Dialog,
    /// Where the dialog's (0, 0) is on the screen.
    pub origin: (f32, f32),
    pub ctls: Vec<Ctl>,
    hover: Option<usize>,
    pressed: Option<usize>,
    list: Option<List>,
    /// The slider being dragged.
    dragging: Option<usize>,
    /// The edit box that has the keyboard.
    editing: Option<usize>,
    /// The pointer, and the control under it since when, for its tip.
    pointer: (f32, f32),
    tip: (Option<usize>, f32),
}

impl Dlg {
    pub fn open(ui: &mut Ui, pack: &str, origin: (f32, f32)) -> Option<Dlg> {
        let dialog = ui.dialog(pack)?;
        let ctls = dialog
            .controls
            .iter()
            .map(|c| Ctl {
                // Pull-downs start closed.
                visible: c.props.flag("Show").unwrap_or(true) && c.get("FX").is_none(),
                enabled: true,
                on: false,
                text: c
                    .props
                    .int("LocalTextId")
                    .and_then(|id| ui.string(id as u32))
                    .map(str::to_string)
                    .or_else(|| c.get("Text").map(str::to_string)),
                items: Vec::new(),
                selected: 0,
                scroll: 0,
                fx: None,
                at: 0.0,
                steps: c
                    .props
                    .int("TickNum")
                    .filter(|&n| n > 0)
                    .map_or(0, |n| n as u32 - 1),
                range: c.props.int("Scale").map_or(100, |n| n.max(0) as u32),
                reversed: false,
                thumb: (0.0, 0.0),
                max_len: c.props.int("MaxLen").map_or(100, |n| n.max(0) as usize),
                allowed: String::new(),
                pending: None,
                mark: None,
                selectable: false,
                wrap: false,
            })
            .collect();
        Some(Dlg {
            pack: pack.to_string(),
            dialog,
            origin,
            ctls,
            hover: None,
            pressed: None,
            list: None,
            dragging: None,
            editing: None,
            pointer: (0.0, 0.0),
            tip: (None, 0.0),
        })
    }

    /// How many controls the dialog has.
    pub fn ctls_len(&self) -> usize {
        self.ctls.len()
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.dialog
            .controls
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }

    pub fn ctl(&mut self, name: &str) -> Option<&mut Ctl> {
        let i = self.index(name)?;
        self.ctls.get_mut(i)
    }

    pub fn get(&self, name: &str) -> Option<&Ctl> {
        self.ctls.get(self.index(name)?)
    }

    pub fn show(&mut self, name: &str, visible: bool) {
        if let Some(c) = self.ctl(name) {
            c.visible = visible;
        }
    }

    pub fn set_text(&mut self, name: &str, text: impl Into<String>) {
        if let Some(c) = self.ctl(name) {
            c.text = Some(text.into());
        }
    }

    /// Changes a control's text height (the game's `FontHeight`).
    pub fn set_font(&mut self, name: &str, height: f32) {
        if let Some(i) = self.index(name) {
            let props = &mut self.dialog.controls[i].props.0;
            props.retain(|(k, _)| !k.eq_ignore_ascii_case("FontHeight"));
            props.push(("FontHeight".into(), height.to_string()));
        }
    }

    pub fn enable(&mut self, name: &str, enabled: bool) {
        if let Some(c) = self.ctl(name) {
            c.enabled = enabled;
        }
    }

    pub fn set_on(&mut self, name: &str, on: bool) {
        if let Some(c) = self.ctl(name) {
            c.on = on;
        }
    }

    /// Fills a list and chooses a line.
    pub fn set_items(&mut self, name: &str, items: Vec<String>, selected: usize) {
        if let Some(c) = self.ctl(name) {
            c.selected = selected.min(items.len().saturating_sub(1));
            c.items = items;
        }
        self.spin(name);
    }

    pub fn select(&mut self, name: &str, selected: usize) {
        if let Some(c) = self.ctl(name) {
            c.selected = selected.min(c.items.len().saturating_sub(1));
        }
        self.spin(name);
    }

    /// A one line list box (the number between a pair of arrows) shows
    /// its chosen line.
    fn spin(&mut self, name: &str) {
        let Some(i) = self.index(name) else {
            return;
        };
        if self.dialog.controls[i].kind == Kind::ListBox && self.rect(i)[3] < 2.0 * self.line(i) {
            self.ctls[i].scroll = self.ctls[i].selected;
        }
    }

    pub fn selected(&self, name: &str) -> usize {
        self.get(name).map_or(0, |c| c.selected)
    }

    /// Shows or hides every control with this `GroupId` or `AttachId`.
    pub fn show_group(&mut self, id: i32, visible: bool) {
        for (c, ctl) in self.dialog.controls.iter().zip(&mut self.ctls) {
            if c.props.int("GroupId") == Some(id) || c.props.int("AttachId") == Some(id) {
                ctl.visible = visible;
            }
        }
    }

    /// Whether a pull-down is all the way down.
    /// Whether a pull-down is out of sight: closed, and done closing.
    pub fn is_shut(&self, name: &str) -> bool {
        self.index(name).is_none_or(|i| !self.ctls[i].visible)
    }

    /// The pictures of every pull-down's animation, to have them ready.
    pub fn fx_frames(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for c in &self.dialog.controls {
            let (Some(first), Some(last)) = (c.get("FXAnimIn"), c.anim("AnimNorm")) else {
                continue;
            };
            let (stem, from) = split_number(first);
            let (_, to) = split_number(last);
            let digits = first.len() - stem.len();
            for n in from..=to {
                out.push((self.pack.clone(), format!("{stem}{n:0digits$}")));
            }
        }
        out
    }

    pub fn is_down(&self, name: &str) -> bool {
        self.index(name)
            .is_some_and(|i| self.ctls[i].visible && self.ctls[i].fx.is_none())
    }

    /// Whether a slider is being dragged.
    pub fn is_dragging(&self, name: &str) -> bool {
        self.dragging.is_some() && self.dragging == self.index(name)
    }

    /// Whether a pull-down is open (or opening).
    pub fn is_open(&self, name: &str) -> bool {
        self.index(name).is_some_and(|i| {
            let c = &self.ctls[i];
            c.visible && c.fx.is_none_or(|f| f.opening)
        })
    }

    /// Opens or closes a pull-down (`FX = Anim`), playing its frames and
    /// its sound; with the UI Animations option off it opens or shuts at
    /// once (the dialog's +0xc8, 0x471590 and 0x472430).
    pub fn pull(&mut self, name: &str, open: bool, now: f32, animate: bool) -> Vec<Event> {
        let Some(i) = self.index(name) else {
            return Vec::new();
        };
        if self.is_open(name) == open {
            return Vec::new();
        }
        let c = &self.dialog.controls[i];
        let sound = c
            .get(if open { "FXSoundIn" } else { "FXSoundOut" })
            .map(|s| Event::Sound(s.to_string()));
        let ctl = &mut self.ctls[i];
        ctl.visible = open || animate;
        ctl.fx = animate.then_some(Fx {
            start: now,
            opening: open,
        });
        sound.into_iter().collect()
    }

    /// Whether the control called `name` and every anchor above it is showing:
    /// for a pull-down's button, that the pull-down is all the way open.
    pub fn is_showing(&self, name: &str) -> bool {
        self.index(name).is_some_and(|i| self.showing(i))
    }

    /// Whether control `i` and every anchor above it is showing.
    fn showing(&self, i: usize) -> bool {
        let mut i = i;
        for depth in 0..8 {
            let c = &self.ctls[i];
            if !c.visible {
                return false;
            }
            // Anchored controls show once their pull-down is fully open.
            if depth > 0 && c.fx.is_some() {
                return false;
            }
            match self.dialog.controls[i]
                .anchor
                .as_deref()
                .and_then(|a| self.index(a))
            {
                Some(a) => i = a,
                None => return true,
            }
        }
        true
    }

    /// The pull-down frame to show for control `i` and whether it is still
    /// moving.
    fn fx_frame(&self, i: usize, now: f32) -> (Option<String>, bool) {
        let c = &self.dialog.controls[i];
        let (Some(first), Some(last)) = (c.get("FXAnimIn"), c.anim("AnimNorm")) else {
            return (c.anim("AnimNorm").map(str::to_string), false);
        };
        let (stem, from) = split_number(first);
        let (_, to) = split_number(last);
        let count = to.saturating_sub(from) + 1;
        let Some(fx) = self.ctls[i].fx else {
            return (Some(last.to_string()), false);
        };
        let step = ((now - fx.start) / FX_FRAME).max(0.0) as u32;
        let moving = step < count;
        let k = step.min(count - 1);
        let n = if fx.opening { from + k } else { to - k };
        let digits = first.len() - stem.len();
        (Some(format!("{stem}{n:0digits$}")), moving)
    }

    /// Ends pull-downs that have finished closing.
    pub fn tick(&mut self, now: f32) {
        for i in 0..self.ctls.len() {
            if let Some(fx) = self.ctls[i].fx {
                let moving = self.fx_frame(i, now).1;
                if !moving {
                    self.ctls[i].fx = None;
                    if !fx.opening {
                        self.ctls[i].visible = false;
                    }
                }
            }
        }
    }

    /// A control's rectangle on the screen.
    pub fn rect(&self, i: usize) -> [f32; 4] {
        let r = self.dialog.place(&self.dialog.controls[i]);
        [
            r[0] as f32 + self.origin.0,
            r[1] as f32 + self.origin.1,
            r[2] as f32,
            r[3] as f32,
        ]
    }

    pub fn rect_of(&self, name: &str) -> Option<[f32; 4]> {
        self.index(name).map(|i| self.rect(i))
    }

    fn clickable(c: &Control) -> bool {
        matches!(
            c.kind,
            Kind::Button
                | Kind::RadioButton
                | Kind::MultiState
                | Kind::ScrollUp
                | Kind::ScrollDown
                | Kind::DropDownList
                | Kind::EditBox
                | Kind::HScrollBar
                | Kind::VScrollBar
                | Kind::ListBox
        )
    }

    /// The control under the pointer, topmost first.
    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.ctls.len()).rev().find(|&i| {
            let c = &self.dialog.controls[i];
            let [rx, ry, rw, rh] = self.rect(i);
            Self::clickable(c)
                && self.ctls[i].enabled
                && self.showing(i)
                && x >= rx
                && y >= ry
                && x < rx + rw
                && y < ry + rh
        })
    }

    /// The line of the open list under the pointer.
    fn list_line(&self, x: f32, y: f32) -> Option<Option<usize>> {
        let l = self.list?;
        let [lx, ly, lw] = l.lines;
        let inside = x >= lx && x < lx + lw && y >= ly && y < ly + l.rows as f32 * LINE;
        if !inside {
            return Some(None);
        }
        let ctl = &self.ctls[l.control];
        let line = ctl.scroll + ((y - ly) / LINE) as usize;
        Some((line < ctl.items.len()).then_some(line))
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) -> Vec<Event> {
        if let Some(line) = self.list_line(x, y) {
            if let Some(l) = &mut self.list {
                l.hover = line;
            }
            return Vec::new();
        }
        let mut events = Vec::new();
        if let Some(i) = self.dragging {
            if self.drag(i, x, y) {
                events.push(Event::Changed(self.dialog.controls[i].name.clone()));
            }
            return events;
        }
        self.pointer = (x, y);
        let hit = self.hit(x, y);
        if hit != self.hover {
            if let Some(s) = hit.and_then(|i| self.dialog.controls[i].get("SoundFocus")) {
                events.push(Event::Sound(s.to_string()));
            }
            self.hover = hit;
        }
        events
    }

    pub fn mouse_down(&mut self, x: f32, y: f32) {
        self.pressed = if self.list.is_some() {
            None
        } else {
            self.hit(x, y)
        };
        let Some(i) = self.pressed else { return };
        if matches!(
            self.dialog.controls[i].kind,
            Kind::HScrollBar | Kind::VScrollBar
        ) {
            // A slider with steps jumps to the pointer (`0x476560`); one
            // without moves only by dragging. Either can then be dragged.
            if self.ctls[i].steps > 0 {
                self.drag(i, x, y);
            }
            self.dragging = Some(i);
        }
    }

    /// Scrolls an open list.
    pub fn wheel(&mut self, lines: i32) {
        if let Some(l) = self.list {
            let ctl = &mut self.ctls[l.control];
            let most = ctl.items.len().saturating_sub(l.rows);
            ctl.scroll = (ctl.scroll as i32 - lines).clamp(0, most as i32) as usize;
        }
    }

    /// Whether a drop-down list is open (it takes the clicks).
    pub fn list_open(&self) -> bool {
        self.list.is_some()
    }

    pub fn close_list(&mut self) {
        self.list = None;
    }

    pub fn mouse_up(&mut self, x: f32, y: f32) -> Vec<Event> {
        let mut events = Vec::new();
        if let Some(i) = self.dragging.take() {
            self.pressed = None;
            let name = self.dialog.controls[i].name.clone();
            events.push(Event::Changed(name.clone()));
            events.push(Event::Released(name));
            return events;
        }
        if let Some(line) = self.list_line(x, y) {
            let l = self.list.take().unwrap();
            if let Some(line) = line {
                let c = &self.dialog.controls[l.control];
                self.ctls[l.control].selected = line;
                events.push(Event::Selected(c.name.clone(), line));
            }
            return events;
        }
        let pressed = self.pressed.take();
        let hit = self.hit(x, y);
        let (Some(p), Some(h)) = (pressed, hit) else {
            return events;
        };
        if p != h {
            return events;
        }
        let c = &self.dialog.controls[h];
        if let Some(s) = c.get("SoundPush") {
            events.push(Event::Sound(s.to_string()));
        }
        match c.kind {
            Kind::MultiState => self.ctls[h].on = !self.ctls[h].on,
            Kind::RadioButton => {
                if let Some(group) = c.props.int("GroupId") {
                    for (o, ctl) in self.dialog.controls.iter().zip(&mut self.ctls) {
                        if o.props.int("GroupId") == Some(group) {
                            ctl.on = false;
                        }
                    }
                }
                self.ctls[h].on = true;
            }
            Kind::DropDownList if !self.ctls[h].items.is_empty() => {
                // Lines are placed when the list is drawn.
                let ctl = &mut self.ctls[h];
                ctl.scroll = ctl.selected.saturating_sub(2);
                self.list = Some(List {
                    control: h,
                    lines: [0.0; 3],
                    rows: 0,
                    hover: None,
                });
            }
            Kind::ListBox => {
                let name = c.name.clone();
                let r = self.rect(h);
                let height = self.line(h);
                let ctl = &mut self.ctls[h];
                let line = ctl.scroll + ((y - r[1]) / height).max(0.0) as usize;
                if ctl.selectable && line < ctl.items.len() {
                    ctl.selected = line;
                    ctl.mark = Some(line);
                    events.push(Event::Selected(name, line));
                }
                return events;
            }
            Kind::ScrollUp | Kind::ScrollDown => {
                let by = if c.kind == Kind::ScrollUp { -1 } else { 1 };
                let (group, name) = (attach(c), c.name.clone());
                self.scroll_group(group, by);
                events.push(Event::Clicked(name));
                return events;
            }
            Kind::EditBox => self.editing = Some(h),
            _ => {}
        }
        if c.kind != Kind::EditBox && self.editing.is_some() {
            self.editing = None;
        }
        events.push(Event::Clicked(c.name.clone()));
        events
    }

    // ---- Sliders (the game's UISlider, `0x475b40`) ----

    /// How far the thumb of control `i` can travel: its size along the
    /// track less the thumb's.
    fn track(&self, i: usize) -> f32 {
        let r = self.dialog.controls[i].rect;
        let ctl = &self.ctls[i];
        if self.dialog.controls[i].kind == Kind::HScrollBar {
            r[2] as f32 - ctl.thumb.0
        } else {
            r[3] as f32 - ctl.thumb.1
        }
        .max(0.0)
    }

    fn length(&self, i: usize) -> f32 {
        let r = self.dialog.controls[i].rect;
        if self.dialog.controls[i].kind == Kind::HScrollBar {
            r[2] as f32
        } else {
            r[3] as f32
        }
    }

    /// The thumb's place as a value out of `range`, rounded down
    /// (`0x4762a0`).
    fn value_at(&self, i: usize, range: u32) -> u32 {
        let ctl = &self.ctls[i];
        let offset = if ctl.reversed {
            (self.length(i) - ctl.at) as i64
        } else {
            ctl.at as i64
        };
        let track = self.track(i) as i64;
        if track == 0 {
            return 0;
        }
        ((offset.max(0) as u64 * range as u64) as f64 / track as f64) as u32
    }

    /// Moves the thumb to show `value` on a scale of `range` (`0x476360`).
    fn put(&mut self, i: usize, value: u32, range: u32) {
        let at = if self.ctls[i].reversed {
            range.saturating_sub(value)
        } else {
            value
        };
        let track = self.track(i);
        self.ctls[i].at = if range == 0 {
            at as f32
        } else {
            ((track.floor() as u64 * at as u64) as f64 / range as f64).min(track as f64) as f32
        };
    }

    /// Moves a slider to the pointer, as dragging does (`0x476900`).
    /// Whether it moved.
    fn drag(&mut self, i: usize, x: f32, y: f32) -> bool {
        let r = self.rect(i);
        let across = self.dialog.controls[i].kind == Kind::HScrollBar;
        let steps = self.ctls[i].steps;
        let thumb = if across {
            self.ctls[i].thumb.0
        } else {
            self.ctls[i].thumb.1
        };
        let mut offset = if across { x - r[0] } else { y - r[1] }.floor();
        if steps == 0 {
            offset -= (thumb / 2.0).floor();
        }
        let offset = offset.clamp(0.0, self.track(i).floor());
        let before = self.ctls[i].at;
        self.ctls[i].at = offset;
        if steps > 0 {
            let v = self.value_at(i, steps);
            self.put(i, v, steps);
        }
        // A slider beside a list scrolls it.
        if let Some(group) = self.follows(i) {
            let span = self.group_span(group);
            let track = self.track(i);
            if track > 0.0 {
                let first = (self.ctls[i].at / track * span as f32).round() as usize;
                self.scroll_group_to(group, first);
            }
        }
        self.ctls[i].at != before
    }

    /// A slider's value, out of its range: the thumb rounded up, scaled
    /// to the track (`0x4764a0`).
    pub fn slider(&self, name: &str) -> u32 {
        let Some(i) = self.index(name) else { return 0 };
        let ctl = &self.ctls[i];
        if let Some((value, range)) = ctl.pending {
            if range == ctl.range || range == 0 {
                return value;
            }
            return (value as u64 * ctl.range as u64 / range.max(1) as u64) as u32;
        }
        let offset = if ctl.reversed {
            (self.length(i) - ctl.at) as u32
        } else {
            ctl.at.ceil() as u32
        };
        let track = self.track(i) as u32;
        if ctl.range == 0 || track == 0 {
            return offset;
        }
        (ctl.range as u64 * offset as u64 / track as u64) as u32
    }

    /// The thumb as a value out of `range`, rounded down.
    pub fn slider_at(&self, name: &str, range: u32) -> u32 {
        let Some(i) = self.index(name) else { return 0 };
        if let Some((value, r)) = self.ctls[i].pending {
            return (value as u64 * range as u64 / r.max(1) as u64) as u32;
        }
        self.value_at(i, range)
    }

    /// Moves the named slider to show `value` on a scale of `range`, or
    /// on its own scale if `range` is `None`.
    pub fn set_slider(&mut self, name: &str, value: u32, range: Option<u32>) {
        let Some(i) = self.index(name) else { return };
        let range = range.unwrap_or(self.ctls[i].range);
        if self.ctls[i].thumb == (0.0, 0.0) {
            // The thumb's size is known once it is drawn.
            self.ctls[i].pending = Some((value, range));
            return;
        }
        self.put(i, value, range);
    }

    /// The values a slider steps through (`0x476470`).
    pub fn set_steps(&mut self, name: &str, count: u32) {
        if let Some(c) = self.ctl(name) {
            c.steps = count.saturating_sub(1);
        }
    }

    // ---- Lists and their scroll bars, grouped by `AttachId` ----

    /// The list group a scroll bar without steps follows.
    fn follows(&self, i: usize) -> Option<i32> {
        let c = &self.dialog.controls[i];
        if c.kind != Kind::VScrollBar || self.ctls[i].steps > 0 {
            return None;
        }
        let group = attach(c)?;
        self.dialog
            .controls
            .iter()
            .any(|o| o.kind == Kind::ListBox && attach(o) == Some(group))
            .then_some(group)
    }

    /// How many lines the group's lists can scroll through.
    fn group_span(&self, group: i32) -> usize {
        self.group_lists(Some(group))
            .map(|i| {
                let rows = (self.rect(i)[3] / self.line(i)).floor() as usize;
                self.ctls[i].items.len().saturating_sub(rows)
            })
            .max()
            .unwrap_or(0)
    }

    fn group_lists(&self, group: Option<i32>) -> impl Iterator<Item = usize> + '_ {
        (0..self.ctls.len()).filter(move |&i| {
            let c = &self.dialog.controls[i];
            c.kind == Kind::ListBox && group.is_some() && attach(c) == group
        })
    }

    fn scroll_group_to(&mut self, group: i32, first: usize) {
        let lists: Vec<usize> = self.group_lists(Some(group)).collect();
        for i in lists {
            let rows = (self.rect(i)[3] / self.line(i)).floor() as usize;
            let ctl = &mut self.ctls[i];
            ctl.scroll = first.min(ctl.items.len().saturating_sub(rows));
        }
        self.follow_lists(group);
    }

    /// Scrolls every list of `group` by `by` lines.
    pub fn scroll_group(&mut self, group: Option<i32>, by: i32) {
        let Some(group) = group else { return };
        let first = self
            .group_lists(Some(group))
            .map(|i| self.ctls[i].scroll)
            .max()
            .unwrap_or(0);
        self.scroll_group_to(group, (first as i32 + by).max(0) as usize);
    }

    /// The group's scroll bars to where its lists are.
    fn follow_lists(&mut self, group: i32) {
        let span = self.group_span(group);
        let first = self
            .group_lists(Some(group))
            .map(|i| self.ctls[i].scroll)
            .max()
            .unwrap_or(0);
        for i in 0..self.ctls.len() {
            if self.follows(i) == Some(group) && Some(i) != self.dragging {
                let track = self.track(i);
                self.ctls[i].at = if span == 0 {
                    0.0
                } else {
                    (first as f32 * track / span as f32).min(track)
                };
            }
        }
    }

    /// Lets the pointer pick a list box's lines.
    pub fn set_wrap(&mut self, name: &str, on: bool) {
        if let Some(c) = self.ctl(name) {
            c.wrap = on;
        }
    }

    /// A list's colour for its picked line, as the game's BGR.
    pub fn set_select_color(&mut self, name: &str, bgr: i32) {
        if let Some(i) = self.index(name) {
            let props = &mut self.dialog.controls[i].props.0;
            props.retain(|(k, _)| !k.eq_ignore_ascii_case("SelectColor"));
            props.push(("SelectColor".into(), bgr.to_string()));
        }
    }

    /// A control's text colour, as the game's BGR (`0x471c70`).
    /// Adds a line of text of the port's own, drawn as control `like` is
    /// (its font, colour and anchor), at `rect` from that anchor and
    /// breaking at its width; once, then only its text changes.
    pub fn add_text(&mut self, like: &str, name: &str, rect: [i32; 4], text: &str) {
        if self.index(name).is_none() {
            let Some(i) = self.index(like) else { return };
            let mut c = self.dialog.controls[i].clone();
            c.name = name.to_string();
            c.rect = rect;
            c.props.0.retain(|(k, _)| !k.eq_ignore_ascii_case("ToolTipText"));
            let mut ctl = self.ctls[i].clone();
            ctl.wrap = true;
            self.dialog.controls.push(c);
            self.ctls.push(ctl);
        }
        self.set_text(name, text);
    }

    pub fn set_color(&mut self, name: &str, bgr: i32) {
        if let Some(i) = self.index(name) {
            let props = &mut self.dialog.controls[i].props.0;
            props.retain(|(k, _)| !k.eq_ignore_ascii_case("TextColor"));
            props.push(("TextColor".into(), bgr.to_string()));
        }
    }

    /// A control's text alignment, as the game's flags (`0x471cd0`, read
    /// by `0x4705f0`): 2 centred across, 4 to the right, 0x10 centred
    /// down, 0x20 to the bottom.
    pub fn set_align(&mut self, name: &str, flags: u32) {
        if let Some(i) = self.index(name) {
            let across = match flags {
                f if f & 2 != 0 => "CENTER",
                f if f & 4 != 0 => "RIGHT",
                _ => "LEFT",
            };
            let down = match flags {
                f if f & 0x10 != 0 => "MIDDLE",
                f if f & 0x20 != 0 => "BOTTOM",
                _ => "TOP",
            };
            let props = &mut self.dialog.controls[i].props.0;
            props.retain(|(k, _)| !k.eq_ignore_ascii_case("TextAlign") && !k.eq_ignore_ascii_case("TextAlignV"));
            props.push(("TextAlign".into(), across.into()));
            props.push(("TextAlignV".into(), down.into()));
        }
    }

    pub fn set_selectable(&mut self, name: &str, on: bool) {
        if let Some(c) = self.ctl(name) {
            c.selectable = on;
        }
    }

    /// Picks a list box's line (or none), as a click would.
    pub fn mark(&mut self, name: &str, line: Option<usize>) {
        if let Some(c) = self.ctl(name) {
            c.mark = line.filter(|&l| l < c.items.len());
            if let Some(l) = c.mark {
                c.selected = l;
            }
        }
    }

    /// The list box line picked, if any.
    pub fn marked(&self, name: &str) -> Option<usize> {
        self.get(name).and_then(|c| c.mark)
    }

    // ---- Edit boxes ----

    /// Gives an edit box the keyboard (or none).
    pub fn edit(&mut self, name: Option<&str>) {
        self.editing = name.and_then(|n| self.index(n));
    }

    pub fn editing(&self) -> Option<&str> {
        self.editing.map(|i| self.dialog.controls[i].name.as_str())
    }

    /// A key typed into the edit box that has the keyboard: letters it
    /// takes up to its length, backspace, and Return or Tab to leave it.
    pub fn type_char(&mut self, c: char) -> Vec<Event> {
        let Some(i) = self.editing else {
            return Vec::new();
        };
        let name = self.dialog.controls[i].name.clone();
        let ctl = &mut self.ctls[i];
        let text = ctl.text.get_or_insert_with(String::new);
        match c {
            '\u{8}' => {
                text.pop();
            }
            '\r' | '\n' | '\t' => {
                self.editing = None;
                return vec![Event::Released(name)];
            }
            c if c >= ' '
                && text.chars().count() < ctl.max_len
                && (ctl.allowed.is_empty()
                    || c.is_ascii_alphanumeric()
                    || ctl.allowed.contains(c)) =>
            {
                text.push(c)
            }
            _ => return Vec::new(),
        }
        vec![Event::Changed(name)]
    }

    /// The image control `i` shows now.
    fn image_for(&self, i: usize, now: f32) -> Option<String> {
        let c = &self.dialog.controls[i];
        let ctl = &self.ctls[i];
        if c.get("FX").is_some() {
            return self.fx_frame(i, now).0;
        }
        let key = match c.kind {
            Kind::MultiState => {
                let state = format!(
                    "{}{}",
                    if ctl.enabled { "State" } else { "StateDisable" },
                    ctl.on as u8
                );
                return c
                    .anim(&state)
                    .or_else(|| c.anim(&format!("State{}", ctl.on as u8)))
                    .map(str::to_string);
            }
            Kind::RadioButton => match (ctl.on, ctl.enabled) {
                (true, true) => "AnimOn",
                (true, false) => "AnimOnDisable",
                (false, true) => "AnimOff",
                (false, false) => "AnimOffDisable",
            },
            _ if !ctl.enabled => "AnimDisable",
            _ if self.pressed == Some(i) && self.hover == Some(i) => "AnimPush",
            _ if self.hover == Some(i) => "AnimFocus",
            _ => "AnimNorm",
        };
        c.anim(key)
            .or_else(|| c.anim("AnimNorm"))
            .map(str::to_string)
    }

    pub fn draw(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        self.draw_under(ui, gfx, now);
        self.draw_list(ui, gfx);
    }

    /// The dialog without its open drop-down list, for a screen that draws
    /// something of its own over the dialog and the list over that.
    pub fn draw_under(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        self.draw_background(ui, gfx);
        self.draw_controls(ui, gfx, now);
    }

    /// Only the dialog's background picture.
    pub fn draw_background(&mut self, ui: &mut Ui, gfx: &mut Gfx) {
        if let Some(bg) = self.dialog.background() {
            if let Some(t) = ui.image(gfx, &self.pack, bg) {
                let fills = gfx.sprites.size(t) == (640, 480) && self.origin == (0.0, 0.0);
                if crate::platform::FILL_MENUS && fills {
                    gfx.sprites.backdrop(t);
                } else {
                    gfx.sprites.image(t, self.origin.0, self.origin.1);
                }
            }
        }
    }

    /// The controls without the background or an open drop-down list,
    /// for a screen that draws something between them (the main menu's
    /// movie).
    pub fn draw_controls(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        self.draw_some(ui, gfx, now, 0..self.ctls.len());
    }

    /// The controls in `range` (by their order in the dialog), for
    /// something drawn among them.
    pub fn draw_some(
        &mut self,
        ui: &mut Ui,
        gfx: &mut Gfx,
        now: f32,
        range: std::ops::Range<usize>,
    ) {
        for i in range {
            if !self.showing(i) {
                continue;
            }
            let c = &self.dialog.controls[i];
            let rect = self.rect(i);
            let mut text_rect = rect;
            let mut boxed = false;
            if matches!(c.kind, Kind::HScrollBar | Kind::VScrollBar) {
                self.draw_slider(ui, gfx, i);
                continue;
            }
            if c.kind == Kind::DropDownList {
                // The text's box at the left, the arrow at the right.
                if let Some(t) = c
                    .anim("AnimTextBack")
                    .and_then(|n| ui.image(gfx, &self.pack, n))
                {
                    let (w, h) = gfx.sprites.size(t);
                    gfx.sprites.image(t, rect[0], rect[1]);
                    text_rect = [rect[0], rect[1], w as f32, h as f32];
                }
                if let Some(t) = self
                    .image_for(i, now)
                    .and_then(|n| ui.image(gfx, &self.pack, &n))
                {
                    let (w, _) = gfx.sprites.size(t);
                    gfx.sprites.image(t, rect[0] + rect[2] - w as f32, rect[1]);
                }
            } else if let Some(t) = self
                .image_for(i, now)
                .and_then(|n| ui.image(gfx, &self.pack, &n))
            {
                let (w, h) = gfx.sprites.size(t);
                gfx.sprites.image(t, rect[0], rect[1]);
                // Check boxes and radio buttons put their text after the box,
                // however it is aligned (the options pages have both); a
                // picture as wide as the control is a button's face.
                boxed = matches!(c.kind, Kind::MultiState | Kind::RadioButton)
                    && c.get("TextAlign") != Some("CENTER")
                    && (w as f32) < rect[2] / 2.0;
                if boxed {
                    text_rect = [
                        rect[0] + w as f32,
                        rect[1],
                        rect[2] - w as f32,
                        rect[3].max(h as f32),
                    ];
                }
            }
            if self.ctls[i].fx.is_some() {
                continue;
            }
            let ctl = &self.ctls[i];
            if c.kind == Kind::ListBox {
                // A line per item, as many as fit.
                let height = self.line(i);
                let rows = (rect[3] / height).floor() as usize;
                let items: Vec<String> = ctl
                    .items
                    .iter()
                    .skip(ctl.scroll)
                    .take(rows)
                    .cloned()
                    .collect();
                for (k, item) in items.iter().enumerate() {
                    let line = [rect[0], rect[1] + k as f32 * height, rect[2], height];
                    // The picked line on its box (`SelectBoxColor`), in
                    // `SelectColor`; a list that names neither puts it on
                    // a light blue box in its own colour (the profiles).
                    let picked = ctl.mark == Some(ctl.scroll + k);
                    let plain = c.props.get("SelectColor").is_none()
                        && c.props.get("SelectBoxColor").is_none();
                    if picked {
                        let b = c.props.int("SelectBoxColor");
                        if let Some(b) = b.or(plain.then_some(SELECT_BOX)) {
                            ui.fill(gfx, line, color(b));
                        }
                    }
                    self.draw_text(ui, gfx, c, line, item, picked && !plain);
                }
                continue;
            }
            let text = match c.kind {
                Kind::DropDownList if !ctl.items.is_empty() => ctl.item(),
                _ => ctl.text.as_deref(),
            };
            let editing = self.editing == Some(i);
            // Static text with room for two lines is drawn as GDI's word
            // break draws it: from the top, whatever `TextAlignV` says.
            let tall = c.kind == Kind::StaticText && {
                let (pt, bold) = self.font(c);
                ui.text(gfx, "Ag", pt, bold)
                    .is_some_and(|t| text_rect[3] >= 2.0 * gfx.sprites.size(t).1 as f32)
            };
            if ctl.wrap || tall {
                let text = text.unwrap_or("").to_string();
                self.draw_wrapped(ui, gfx, c, text_rect, &text);
                continue;
            }
            if let Some(text) = text.filter(|t| !t.is_empty() || editing) {
                let mut text = text.to_string();
                if editing && (now * 2.0).fract() < 0.5 {
                    text.push('|');
                }
                // Only a control with a SelectColor lights its text under
                // the pointer; the rest keep theirs (the captures' Easy,
                // Race Statistics and Advanced...).
                let focus = self.hover == Some(i)
                    && c.props.int("SelectColor").is_some()
                    && !matches!(
                        c.kind,
                        Kind::DropDownList | Kind::EditBox | Kind::StaticText
                    );
                if boxed {
                    // The original centres a box's text on it, whatever
                    // `TextAlignV` says.
                    let mut middle = c.clone();
                    middle
                        .props
                        .0
                        .retain(|(k, _)| !k.eq_ignore_ascii_case("TextAlignV"));
                    middle.props.0.push(("TextAlignV".into(), "MIDDLE".into()));
                    self.draw_text(ui, gfx, &middle, text_rect, &text, focus);
                } else if c.kind == Kind::Button
                    && !self.ctls[i].enabled
                    && c.props.int("TextColor").unwrap_or(0xffffff) == 0xffffff
                {
                    // A greyed button's white text greys too (the empty
                    // name box's OK).
                    let mut grey = c.clone();
                    grey.props
                        .0
                        .retain(|(k, _)| !k.eq_ignore_ascii_case("TextColor"));
                    grey.props.0.push(("TextColor".into(), GREYED.to_string()));
                    self.draw_text(ui, gfx, &grey, text_rect, &text, false);
                } else {
                    self.draw_text(ui, gfx, c, text_rect, &text, focus);
                }
            } else if editing && (now * 2.0).fract() < 0.5 {
                self.draw_text(ui, gfx, c, text_rect, "|", false);
            }
        }
    }

    /// Text broken into lines at words to fit the control's width, and
    /// at its own line breaks.
    fn draw_wrapped(&self, ui: &mut Ui, gfx: &mut Gfx, c: &Control, rect: [f32; 4], text: &str) {
        let (pt, bold) = self.font(c);
        let mut lines: Vec<String> = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split(' ') {
                let tried = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{line} {word}")
                };
                if !line.is_empty() && ui.text_width(gfx, &tried, pt, bold) > rect[2] {
                    lines.push(std::mem::take(&mut line));
                    line = word.to_string();
                } else {
                    line = tried;
                }
            }
            lines.push(line);
        }
        let height = ui
            .text(gfx, "Ag", pt, bold)
            .map_or(pt.ceil(), |t| gfx.sprites.size(t).1 as f32);
        for (k, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let r = [rect[0], rect[1] + k as f32 * height, rect[2], height];
            let mut top = c.clone();
            top.props
                .0
                .retain(|(k, _)| !k.eq_ignore_ascii_case("TextAlignV"));
            self.draw_text(ui, gfx, &top, r, line, false);
        }
    }

    /// How far apart a list box's lines are: its font's cell (Arial's
    /// ascent and descent, each to whole pixels) and one more, which
    /// puts the controls page's small font 13 apart as the original does.
    fn line(&self, i: usize) -> f32 {
        let (pt, _) = self.font(&self.dialog.controls[i]);
        let em = em_of(pt);
        (em * 0.905).round() + (em * 0.212).round() + 1.0
    }

    /// A control's text height and weight.
    fn font(&self, c: &Control) -> (f32, bool) {
        let font = |key: &str| {
            c.get(key)
                .or_else(|| self.dialog.info.get(key))
                .map(str::to_string)
        };
        let pt = font("FontHeight")
            .and_then(|h| h.parse().ok())
            .unwrap_or(14.25);
        let bold = font("Bold")
            .and_then(|b| b.parse::<i32>().ok())
            .unwrap_or(700)
            >= 600;
        (pt, bold)
    }

    /// Draws a slider: the track, centred in the control's box, and the
    /// thumb over it (`0x475e70`).
    fn draw_slider(&mut self, ui: &mut Ui, gfx: &mut Gfx, i: usize) {
        let c = &self.dialog.controls[i];
        let r = self.rect(i);
        let across = c.kind == Kind::HScrollBar;
        if let Some(t) = c
            .anim("AnimNorm")
            .and_then(|n| ui.image(gfx, &self.pack, n))
        {
            let (w, h) = gfx.sprites.size(t);
            let (x, y) = if across {
                (r[0], r[1] + ((r[3] - h as f32) / 2.0).max(0.0).floor())
            } else {
                (r[0] + ((r[2] - w as f32) / 2.0).max(0.0).floor(), r[1])
            };
            gfx.sprites.image(t, x, y);
        }
        let enabled = self.ctls[i].enabled;
        let thumb = c
            .anim(if enabled {
                "AnimThumb"
            } else {
                "AnimThumbDisable"
            })
            .or_else(|| c.anim("AnimThumb"))
            .and_then(|n| ui.image(gfx, &self.pack, n));
        let Some(t) = thumb else { return };
        // A list's bar has no thumb while its lines all fit (the replays,
        // the records, the Pro-Circuit careers), unless it has arrows too
        // (the profiles).
        let arrows = |g| {
            self.dialog.controls.iter().any(|o| {
                matches!(o.kind, Kind::ScrollUp | Kind::ScrollDown) && attach(o) == Some(g)
            })
        };
        if self
            .follows(i)
            .is_some_and(|g| self.group_span(g) == 0 && !arrows(g))
        {
            return;
        }
        let (w, h) = gfx.sprites.size(t);
        if self.ctls[i].thumb == (0.0, 0.0) {
            self.ctls[i].thumb = (w as f32, h as f32);
            if let Some((value, range)) = self.ctls[i].pending.take() {
                self.put(i, value, range);
            }
            if let Some(group) = self.follows(i) {
                self.follow_lists(group);
            }
        }
        let at = self.ctls[i].at.floor();
        let (x, y) = if across {
            (r[0] + at, r[1] + ((r[3] - h as f32) / 2.0).floor())
        } else {
            (r[0] + ((r[2] - w as f32) / 2.0).floor(), r[1] + at)
        };
        gfx.sprites.image(t, x, y);
    }

    /// Whether the control under the pointer has a tip to show.
    pub fn has_tip(&self) -> bool {
        self.hover
            .is_some_and(|i| tip_text(&self.dialog.controls[i]).is_some())
    }

    /// The tip (`ToolTipText`) of the control the pointer has rested on
    /// for a moment, in the game's small Arial (`CreateFont` 12, the GUI
    /// manager's), on a dark box under the pointer.
    /// Keeps time on every dialog, and draws only where `show`.
    pub fn draw_tip(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32, show: bool) {
        if self.tip.0 != self.hover {
            self.tip = (self.hover, now);
            return;
        }
        if !show || self.pressed.is_some() || self.list.is_some() || now - self.tip.1 < TIP_DELAY {
            return;
        }
        let Some(text) = self.hover.and_then(|i| tip_text(&self.dialog.controls[i])) else {
            return;
        };
        let Some(t) = ui.tip_text(gfx, &text, TIP_EM) else {
            return;
        };
        // As captured from the original: an opaque grey box 14 high with a
        // light rim, centred under the pointer 16 below its tip and kept on
        // screen, the text's ink 2 in from the left.
        let (tw, th) = gfx.sprites.size(t);
        let (tw, th) = (tw as f32, th as f32);
        let (w, h) = (tw + 2.0, 14.0);
        let x = (self.pointer.0 - w / 2.0).round().min(640.0 - w).max(0.0);
        let mut y = (self.pointer.1 + 16.0).round();
        if y + h > 480.0 {
            y = (self.pointer.1 - h - 2.0).round();
        }
        ui.fill(gfx, [x, y, w, h], color(0x838183));
        ui.fill(gfx, [x + 1.0, y + 1.0, w - 2.0, h - 2.0], color(0x414041));
        gfx.sprites.quad(
            t,
            [0.0, 0.0, 1.0, 1.0],
            [x + 1.0, y + ((h - th) / 2.0).floor(), tw, th],
            color(0xcdcecd),
        );
        gfx.sprites.anchor_text(1, (x + w / 2.0, 0.5), (y + h / 2.0, 0.5));
    }

    /// An open drop-down list, over everything else.
    pub fn draw_list(&mut self, ui: &mut Ui, gfx: &mut Gfx) {
        let Some(mut l) = self.list else { return };
        let c = &self.dialog.controls[l.control];
        let rect = self.rect(l.control);
        let top_h = c
            .anim("AnimTextBack")
            .and_then(|n| ui.image(gfx, &self.pack, n))
            .map_or(rect[3], |t| gfx.sprites.size(t).1 as f32);
        let (x, y) = (rect[0], rect[1] + top_h);
        let (mut w, mut h) = (rect[2], LINE * 6.0);
        if let Some(t) = c
            .anim("AnimListBack")
            .and_then(|n| ui.image(gfx, &self.pack, n))
        {
            let (tw, th) = gfx.sprites.size(t);
            (w, h) = (tw as f32, th as f32);
            gfx.sprites.image(t, x, y);
        } else {
            ui.fill(gfx, [x, y, w, h], [0.0, 0.0, 0.0, 0.85]);
        }
        let rows = ((h - 2.0) / LINE).floor().max(1.0) as usize;
        let ctl = &mut self.ctls[l.control];
        ctl.scroll = ctl.scroll.min(ctl.items.len().saturating_sub(rows));
        let ctl = &self.ctls[l.control];
        let mut lines_w = w;
        if ctl.items.len() > rows {
            if let Some(t) = c
                .anim("AnimScrollBack")
                .and_then(|n| ui.image(gfx, &self.pack, n))
            {
                let (sw, sh) = gfx.sprites.size(t);
                lines_w = w - sw as f32;
                gfx.sprites.image(t, x + lines_w, y);
                if let Some(thumb) = c
                    .anim("AnimThumb")
                    .and_then(|n| ui.image(gfx, &self.pack, n))
                {
                    let (tw, th) = gfx.sprites.size(thumb);
                    let travel = (sh as f32 - th as f32).max(0.0);
                    let most = (ctl.items.len() - rows).max(1) as f32;
                    let ty = y + travel * ctl.scroll as f32 / most;
                    gfx.sprites
                        .image(thumb, x + lines_w + (sw as f32 - tw as f32) / 2.0, ty);
                }
            }
        }
        l.lines = [x, y + 1.0, lines_w];
        l.rows = rows;
        self.list = Some(l);
        let items: Vec<(usize, String)> = ctl
            .items
            .iter()
            .enumerate()
            .skip(ctl.scroll)
            .take(rows)
            .map(|(i, s)| (i, s.clone()))
            .collect();
        for (k, (i, item)) in items.into_iter().enumerate() {
            let line = [x, y + 1.0 + k as f32 * LINE, lines_w, LINE];
            let mut left = c.clone();
            left.props
                .0
                .retain(|(k, _)| !k.eq_ignore_ascii_case("TextAlignV"));
            left.props.0.push(("TextAlignV".into(), "MIDDLE".into()));
            self.draw_text(ui, gfx, &left, line, &item, l.hover == Some(i));
        }
    }

    fn draw_text(
        &self,
        ui: &mut Ui,
        gfx: &mut Gfx,
        c: &Control,
        rect: [f32; 4],
        text: &str,
        focus: bool,
    ) {
        let font = |key: &str| {
            c.get(key)
                .or_else(|| self.dialog.info.get(key))
                .map(str::to_string)
        };
        let pt = font("FontHeight")
            .and_then(|h| h.parse().ok())
            .unwrap_or(14.25);
        let bold = font("Bold")
            .and_then(|b| b.parse::<i32>().ok())
            .unwrap_or(700)
            >= 600;
        let italic = font("Italic")
            .and_then(|b| b.split('\'').next()?.trim().parse::<i32>().ok())
            .is_some_and(|i| i != 0);
        let Some(t) = ui.text_in(gfx, text, pt, bold, italic) else {
            return;
        };
        let (w, h) = gfx.sprites.size(t);
        let (w, h) = (w as f32, h as f32);
        let lm = c.props.int("TextLMargin").unwrap_or(0) as f32;
        let tm = c.props.int("TextTMargin").unwrap_or(0) as f32;
        let x = match c.get("TextAlign").unwrap_or("LEFT") {
            "CENTER" => rect[0] + (rect[2] - w) / 2.0,
            "RIGHT" if !matches!(c.kind, Kind::MultiState | Kind::RadioButton) => {
                rect[0] + rect[2] - w - lm
            }
            _ => rect[0] + lm,
        };
        let y = match c.get("TextAlignV").unwrap_or("TOP") {
            "MIDDLE" => rect[1] + (rect[3] - h) / 2.0,
            // The top margin keeps bottom text off the edge too (the tabs).
            "BOTTOM" => rect[1] + rect[3] - h - tm,
            _ => rect[1] + tm,
        };
        let colour = if focus {
            c.props
                .int("SelectColor")
                .map(color)
                .unwrap_or([1.0, 0.85, 0.2, 1.0])
        } else {
            color(c.props.int("TextColor").unwrap_or(0xffffff))
        };
        if c.props.flag("TextDrop").unwrap_or(false) {
            let drop = color(c.props.int("DropColor").unwrap_or(0));
            gfx.sprites.quad(
                t,
                [0.0, 0.0, 1.0, 1.0],
                [x.round() + 1.0, y.round() + 1.0, w, h],
                drop,
            );
        }
        gfx.sprites.quad(
            t,
            [0.0, 0.0, 1.0, 1.0],
            [x.round(), y.round(), w, h],
            colour,
        );
        // In a wide window the text keeps its shape where its box stretches:
        // it stays at the box's left, middle or right, as it was set.
        let shadow = c.props.flag("TextDrop").unwrap_or(false) as usize;
        let across = match c.get("TextAlign").unwrap_or("LEFT") {
            "CENTER" => (rect[0] + rect[2] / 2.0, 0.5),
            "RIGHT" if !matches!(c.kind, Kind::MultiState | Kind::RadioButton) => (rect[0] + rect[2] - lm, 1.0),
            _ => (rect[0] + lm, 0.0),
        };
        let down = match c.get("TextAlignV").unwrap_or("TOP") {
            "MIDDLE" => (rect[1] + rect[3] / 2.0, 0.5),
            "BOTTOM" => (rect[1] + rect[3] - tm, 1.0),
            _ => (rect[1] + tm, 0.0),
        };
        gfx.sprites.anchor_text(1 + shadow, across, down);
    }
}

/// The list group a control belongs to (`AttachId`).
fn attach(c: &Control) -> Option<i32> {
    c.props.int("AttachId").filter(|&a| a != 0)
}

/// `MultiPullDown0015` → (`MultiPullDown`, 15).
fn split_number(name: &str) -> (&str, u32) {
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (stem, n) = name.split_at(name.len() - digits);
    (stem, n.parse().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_colours() {
        assert_eq!(split_number("SinglePullDown0015"), ("SinglePullDown", 15));
        assert_eq!(
            color(16693626),
            [
                0x7a as f32 / 255.0,
                0xb9 as f32 / 255.0,
                0xfe as f32 / 255.0,
                1.0
            ]
        );
    }
}
