//! snip — region screenshot with quick annotations, and region OCR, for KDE Plasma (Wayland).
//!
//!   snip shot   click a window (or the desktop for the whole screen), or drag a region; it is
//!               copied to the clipboard as PNG. Press 1 (numbered markers), 2 (arrow) or 3 (free draw) before releasing to
//!               annotate first; then Enter / Ctrl+C copies, Ctrl+S also saves, Ctrl+Z undoes.
//!   snip ocr    same selection; the text in it is recognised (tesseract) and copied.
//!
//! The screen is frozen with `spectacle -b` and shown in wlr-layer-shell overlays, one per output.
//! Window outlines for click-to-snap come from a throwaway KWin script that reports back over D-Bus.

use ab_glyph::{Font, FontVec, OutlineCurve, ScaleFont};
use image::{GrayImage, RgbaImage, imageops};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        client::{
            Connection, QueueHandle,
            globals::registry_queue_init,
            protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
        },
        protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler, cursor_shape::CursorShapeManager},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use std::io::Write;
use std::process::{Command, Stdio};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path as SkPath, PathBuilder, Pixmap, PixmapMut, Stroke, Transform};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const RED: [u8; 3] = [235, 52, 52];
const WHITE: [u8; 3] = [255, 255, 255];
const ACCENT: [u8; 3] = [70, 155, 255];
const DIM: u32 = 140; // brightness of the unselected area, out of 256

fn die(msg: &str) -> ! {
    eprintln!("snip: {msg}");
    let _ = Command::new("notify-send").args(["-a", "snip", "-i", "dialog-error", "snip", msg]).status();
    std::process::exit(1)
}

// ---------------------------------------------------------------- model

#[derive(Clone, Copy, PartialEq)]
enum Purpose {
    Shot,
    Ocr,
}
#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Marker,
    Arrow,
    Pen,
}
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    Selecting,
    Annotating,
}
#[derive(Clone, Copy, PartialEq)]
enum Outcome {
    Cancel,
    Copy,
    Save,
}

type Pt = (f32, f32);

#[derive(Clone)]
enum Ann {
    Marker(Pt, u32),
    Arrow(Pt, Pt),
    Pen(Vec<Pt>),
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}
impl Rect {
    /// Pixel-aligned rectangle spanning two points.
    fn span(a: Pt, b: Pt) -> Rect {
        let (x0, x1) = (a.0.min(b.0).floor(), a.0.max(b.0).ceil());
        let (y0, y1) = (a.1.min(b.1).floor(), a.1.max(b.1).ceil());
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
    fn contains(&self, p: Pt) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }
    fn intersect(&self, o: &Rect) -> Option<Rect> {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = ((self.x + self.w).min(o.x + o.w), (self.y + self.h).min(o.y + o.h));
        (x1 - x0 >= 1.0 && y1 - y0 >= 1.0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

struct Ui {
    purpose: Purpose,
    phase: Phase,
    start: Pt,
    cur: Pt,
    sel: Option<Rect>,
    dragged: bool,
    windows: Vec<Rect>, // topmost first
    outputs: Vec<Rect>,
    tool: Option<Tool>,
    anns: Vec<Ann>,
    drawing: Option<Ann>,
    ctrl: bool,
    outcome: Option<Outcome>,
}

impl Ui {
    fn next_marker(&self) -> u32 {
        1 + self.anns.iter().filter(|a| matches!(a, Ann::Marker(..))).count() as u32
    }

    /// What a click at `p` would take: the topmost window there, else the whole output.
    fn snap_at(&self, p: Pt) -> Option<Rect> {
        self.windows.iter().chain(&self.outputs).find(|r| r.contains(p)).copied()
    }
}

// ---------------------------------------------------------------- text & shapes

fn load_font() -> Option<FontVec> {
    let fc = Command::new("fc-match").args(["-f", "%{file}", "sans:bold"]).output().ok();
    let mut paths: Vec<String> = fc.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).into_iter().collect();
    paths.extend(["/usr/share/fonts/noto/NotoSans-Bold.ttf", "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf"].map(String::from));
    paths.iter().find_map(|p| FontVec::try_from_vec(std::fs::read(p).ok()?).ok())
}

/// Outline of `s` with its baseline starting at (x, y), plus its advance width.
fn text_path(font: &FontVec, s: &str, size: f32, x: f32, y: f32) -> (Option<SkPath>, f32) {
    let sf = font.as_scaled(size);
    let (hs, vs) = (sf.h_scale_factor(), sf.v_scale_factor());
    let mut pb = PathBuilder::new();
    let mut caret = 0.0;
    let mut prev = None;
    for c in s.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        if let Some(outline) = font.outline(id) {
            let map = |p: ab_glyph::Point| (x + caret + p.x * hs, y - p.y * vs);
            let mut last: Option<(f32, f32)> = None;
            for curve in &outline.curves {
                let (p0, end) = match curve {
                    OutlineCurve::Line(a, b) => (map(*a), map(*b)),
                    OutlineCurve::Quad(a, _, c) => (map(*a), map(*c)),
                    OutlineCurve::Cubic(a, _, _, d) => (map(*a), map(*d)),
                };
                if last != Some(p0) {
                    if last.is_some() {
                        pb.close();
                    }
                    pb.move_to(p0.0, p0.1);
                }
                match curve {
                    OutlineCurve::Line(..) => pb.line_to(end.0, end.1),
                    OutlineCurve::Quad(_, b, _) => {
                        let b = map(*b);
                        pb.quad_to(b.0, b.1, end.0, end.1)
                    }
                    OutlineCurve::Cubic(_, b, c, _) => {
                        let (b, c) = (map(*b), map(*c));
                        pb.cubic_to(b.0, b.1, c.0, c.1, end.0, end.1)
                    }
                }
                last = Some(end);
            }
            if last.is_some() {
                pb.close();
            }
        }
        caret += sf.h_advance(id);
        prev = Some(id);
    }
    (pb.finish(), caret)
}

/// `swap` draws into BGRA (wl_shm ARGB8888) memory instead of RGBA.
fn paint(c: [u8; 3], alpha: u8, swap: bool) -> Paint<'static> {
    let mut p = Paint::default();
    let (r, b) = if swap { (c[2], c[0]) } else { (c[0], c[2]) };
    p.set_color_rgba8(r, c[1], b, alpha);
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

fn draw_ann(pm: &mut PixmapMut, a: &Ann, tf: Transform, swap: bool, font: Option<&FontVec>) {
    let red = paint(RED, 255, swap);
    match a {
        Ann::Marker((x, y), n) => {
            let r = 15.0;
            if let Some(c) = PathBuilder::from_circle(*x, *y, r) {
                pm.fill_path(&c, &red, FillRule::Winding, tf, None);
                pm.stroke_path(&c, &paint(WHITE, 255, swap), &stroke(2.5), tf, None);
            }
            if let Some(f) = font
                && let (Some(p), _) = text_path(f, &n.to_string(), 18.0, 0.0, 0.0)
            {
                let b = p.bounds();
                let centre = Transform::from_translate(x - (b.left() + b.right()) / 2.0, y - (b.top() + b.bottom()) / 2.0);
                if let Some(p) = p.transform(centre) {
                    pm.fill_path(&p, &paint(WHITE, 255, swap), FillRule::Winding, tf, None);
                }
            }
        }
        Ann::Arrow(a, b) => {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 2.0 {
                return;
            }
            let (ux, uy) = (dx / len, dy / len);
            let head = 22f32.min(len);
            let base = (b.0 - ux * head, b.1 - uy * head);
            if len > head {
                let mut pb = PathBuilder::new();
                pb.move_to(a.0, a.1);
                pb.line_to(base.0 + ux, base.1 + uy);
                if let Some(p) = pb.finish() {
                    pm.stroke_path(&p, &red, &stroke(5.0), tf, None);
                }
            }
            let hw = head * 0.5;
            let mut pb = PathBuilder::new();
            pb.move_to(b.0, b.1);
            pb.line_to(base.0 - uy * hw, base.1 + ux * hw);
            pb.line_to(base.0 + uy * hw, base.1 - ux * hw);
            pb.close();
            if let Some(p) = pb.finish() {
                pm.fill_path(&p, &red, FillRule::Winding, tf, None);
            }
        }
        Ann::Pen(pts) => {
            let mut pb = PathBuilder::new();
            pb.move_to(pts[0].0, pts[0].1);
            for p in &pts[1..] {
                pb.line_to(p.0, p.1);
            }
            if pts.len() == 1 {
                pb.line_to(pts[0].0 + 0.1, pts[0].1);
            }
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &red, &stroke(4.0), tf, None);
            }
        }
    }
}

/// A row of text segments in a dark pill, centred horizontally at `cx`; `true` segments are highlighted.
fn draw_hint(pm: &mut PixmapMut, font: &FontVec, segs: &[(&str, bool)], cx: f32, y: f32) {
    let size = 15.0;
    let gap = 18.0;
    let widths: Vec<f32> = segs.iter().map(|(s, _)| text_path(font, s, size, 0.0, 0.0).1).collect();
    let total = widths.iter().sum::<f32>() + gap * (segs.len() as f32 - 1.0);
    let (pad, h) = (16.0, 34.0);
    let x0 = (cx - total / 2.0).round();
    if let Some(r) = tiny_skia::Rect::from_xywh(x0 - pad, y, total + 2.0 * pad, h) {
        let mut pb = PathBuilder::new();
        pb.push_rect(r);
        if let Some(p) = pb.finish() {
            pm.fill_path(&p, &paint([24, 24, 28], 225, true), FillRule::Winding, Transform::identity(), None);
        }
    }
    let mut x = x0;
    for ((s, hi), w) in segs.iter().zip(&widths) {
        if let (Some(p), _) = text_path(font, s, size, x, y + h / 2.0 + size * 0.36) {
            let c = if *hi { paint(ACCENT, 255, true) } else { paint([215, 215, 220], 255, true) };
            pm.fill_path(&p, &c, FillRule::Winding, Transform::identity(), None);
        }
        x += w + gap;
    }
}

// ---------------------------------------------------------------- wayland overlay

struct Screen {
    layer: LayerSurface,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    bright: Vec<u8>, // BGRA, the frozen screen
    dim: Vec<u8>,
    pool: SlotPool,
    configured: bool,
    frame_pending: bool,
    dirty: bool,
}

struct App {
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    shm: Shm,
    cursor_shape: Option<CursorShapeManager>,
    shape_device: Option<WpCursorShapeDeviceV1>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    screens: Vec<Screen>,
    font: Option<FontVec>,
    ui: Ui,
}

fn draw_screen(s: &mut Screen, ui: &Ui, font: Option<&FontVec>, qh: &QueueHandle<App>) {
    s.dirty = false;
    let (w, h) = (s.w as i32, s.h as i32);
    let Ok((buffer, canvas)) = s.pool.create_buffer(w, h, w * 4, wl_shm::Format::Argb8888) else { return };
    canvas.copy_from_slice(&s.dim);
    let sx = s.x as f32;
    let sy = s.y as f32;
    let local = Transform::from_translate(-sx, -sy);
    if let Some(r) = ui.sel {
        let x0 = ((r.x - sx).max(0.0) as usize).min(s.w as usize);
        let x1 = ((r.x + r.w - sx).max(0.0) as usize).min(s.w as usize);
        let y0 = ((r.y - sy).max(0.0) as usize).min(s.h as usize);
        let y1 = ((r.y + r.h - sy).max(0.0) as usize).min(s.h as usize);
        let stride = s.w as usize * 4;
        for y in y0..y1 {
            let o = y * stride;
            canvas[o + x0 * 4..o + x1 * 4].copy_from_slice(&s.bright[o + x0 * 4..o + x1 * 4]);
        }
    }
    let Some(mut pm) = PixmapMut::from_bytes(canvas, s.w, s.h) else { return };
    if let Some(r) = ui.sel
        && let Some(rect) = tiny_skia::Rect::from_xywh(r.x - 0.5, r.y - 0.5, r.w + 1.0, r.h + 1.0)
    {
        let mut pb = PathBuilder::new();
        pb.push_rect(rect);
        if let Some(p) = pb.finish() {
            let st = Stroke { width: 1.5, ..Stroke::default() };
            pm.stroke_path(&p, &paint(ACCENT, 255, true), &st, local, None);
        }
        if let Some(f) = font {
            let label = format!("{} × {}", r.w as u32, r.h as u32);
            let ly = if r.y - sy > 26.0 { r.y - 9.0 } else { r.y + 20.0 };
            if let (Some(p), _) = text_path(f, &label, 14.0, r.x + 2.0, ly) {
                let shadow = Transform::from_translate(1.0 - sx, 1.0 - sy);
                pm.fill_path(&p, &paint([0, 0, 0], 200, true), FillRule::Winding, shadow, None);
                pm.fill_path(&p, &paint(WHITE, 255, true), FillRule::Winding, local, None);
            }
        }
    }
    for a in ui.anns.iter().chain(ui.drawing.as_ref()) {
        draw_ann(&mut pm, a, local, true, font);
    }
    let here = ui.cur.0 >= sx && ui.cur.0 < sx + s.w as f32 && ui.cur.1 >= sy && ui.cur.1 < sy + s.h as f32;
    if here && let Some(f) = font {
        let t = ui.tool;
        let segs: Vec<(&str, bool)> = match (ui.purpose, ui.phase) {
            (Purpose::Ocr, _) => vec![("click window or drag region to read text", false), ("esc cancel", false)],
            (Purpose::Shot, Phase::Annotating) => vec![
                ("1 markers", t == Some(Tool::Marker)),
                ("2 arrow", t == Some(Tool::Arrow)),
                ("3 draw", t == Some(Tool::Pen)),
                ("ctrl+z undo", false),
                ("enter copy", false),
                ("ctrl+s save", false),
                ("esc cancel", false),
            ],
            (Purpose::Shot, _) => vec![
                ("click window or drag region to copy", t.is_none()),
                ("or press 1 / 2 / 3 first to annotate:", t.is_some()),
                ("1 markers", t == Some(Tool::Marker)),
                ("2 arrow", t == Some(Tool::Arrow)),
                ("3 draw", t == Some(Tool::Pen)),
                ("esc cancel", false),
            ],
        };
        draw_hint(&mut pm, f, &segs, s.w as f32 / 2.0, s.h as f32 - 70.0);
    }
    let surf = s.layer.wl_surface();
    surf.damage_buffer(0, 0, w, h);
    surf.frame(qh, FrameCallbackData(surf.clone()));
    s.frame_pending = true;
    if buffer.attach_to(surf).is_ok() {
        s.layer.commit();
    }
}

impl App {
    fn redraw(&mut self, qh: &QueueHandle<App>) {
        for s in &mut self.screens {
            s.dirty = true;
            if s.configured && !s.frame_pending {
                draw_screen(s, &self.ui, self.font.as_ref(), qh);
            }
        }
    }

    fn screen_of(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.screens.iter().position(|s| s.layer.wl_surface() == surface)
    }

    fn finish_selection(&mut self) {
        let ui = &mut self.ui;
        let r = if ui.dragged { Some(Rect::span(ui.start, ui.cur)) } else { ui.snap_at(ui.cur) };
        let Some(r) = r.filter(|r| r.w >= 4.0 && r.h >= 4.0) else {
            ui.phase = Phase::Idle;
            ui.sel = ui.snap_at(ui.cur);
            return;
        };
        ui.sel = Some(r);
        if ui.purpose == Purpose::Shot && ui.tool.is_some() {
            ui.phase = Phase::Annotating;
        } else {
            ui.outcome = Some(Outcome::Copy);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if let Some(i) = self.screen_of(surface) {
            let s = &mut self.screens[i];
            s.frame_pending = false;
            if s.dirty {
                draw_screen(s, &self.ui, self.font.as_ref(), qh);
            }
        }
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.ui.outcome = Some(Outcome::Cancel);
    }
    fn configure(&mut self, _: &Connection, qh: &QueueHandle<Self>, layer: &LayerSurface, cfg: LayerSurfaceConfigure, _: u32) {
        let Some(i) = self.screen_of(layer.wl_surface()) else { return };
        let s = &mut self.screens[i];
        if cfg.new_size.0 > 0 && (cfg.new_size.0, cfg.new_size.1) != (s.w, s.h) {
            eprintln!("snip: output {}x{} configured as {}x{}", s.w, s.h, cfg.new_size.0, cfg.new_size.1);
        }
        if !s.configured {
            s.configured = true;
            draw_screen(s, &self.ui, self.font.as_ref(), qh);
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seats.get_keyboard(qh, &seat, None).ok();
        }
        if cap == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seats.get_pointer(qh, &seat).ok();
            if let (Some(m), Some(p)) = (&self.cursor_shape, &self.pointer) {
                self.shape_device = Some(m.get_shape_device(p, qh));
            }
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, _: Capability) {}
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {}
    fn press_key(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, ev: KeyEvent) {
        let ui = &mut self.ui;
        let tool = match ev.keysym {
            Keysym::_1 | Keysym::KP_1 => Some(Tool::Marker),
            Keysym::_2 | Keysym::KP_2 => Some(Tool::Arrow),
            Keysym::_3 | Keysym::KP_3 => Some(Tool::Pen),
            _ => None,
        };
        let finish = if ui.phase == Phase::Annotating { Some(Outcome::Copy) } else { None };
        match ev.keysym {
            Keysym::Escape => ui.outcome = Some(Outcome::Cancel),
            _ if tool.is_some() && ui.purpose == Purpose::Shot => {
                ui.tool = tool;
                ui.drawing = None;
            }
            Keysym::z | Keysym::Z if ui.ctrl => {
                if ui.drawing.take().is_none() {
                    ui.anns.pop();
                }
            }
            Keysym::Return | Keysym::KP_Enter => ui.outcome = finish,
            Keysym::c | Keysym::C if ui.ctrl => ui.outcome = finish,
            Keysym::s | Keysym::S if ui.ctrl && finish.is_some() => ui.outcome = Some(Outcome::Save),
            _ => return,
        }
        self.redraw(qh);
    }
    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, m: Modifiers, _: RawModifiers, _: u32) {
        self.ui.ctrl = m.ctrl;
    }
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        let mut changed = false;
        for ev in events {
            let Some(i) = self.screen_of(&ev.surface) else { continue };
            let pos = (self.screens[i].x as f32 + ev.position.0 as f32, self.screens[i].y as f32 + ev.position.1 as f32);
            let ui = &mut self.ui;
            let moved = ui.cur != pos;
            ui.cur = pos;
            match ev.kind {
                PointerEventKind::Enter { serial } => {
                    if let Some(d) = &self.shape_device {
                        d.set_shape(serial, Shape::Crosshair);
                    }
                    if ui.phase == Phase::Idle {
                        ui.sel = ui.snap_at(pos);
                    }
                    changed = true;
                }
                PointerEventKind::Motion { .. } if moved => {
                    match (&ui.phase, &mut ui.drawing) {
                        (Phase::Idle, _) => ui.sel = ui.snap_at(pos),
                        (Phase::Selecting, _) => {
                            // a small wobble during a click still counts as a click on the window
                            ui.dragged |= (pos.0 - ui.start.0).abs() >= 4.0 || (pos.1 - ui.start.1).abs() >= 4.0;
                            if ui.dragged {
                                ui.sel = Some(Rect::span(ui.start, pos));
                            }
                        }
                        (Phase::Annotating, Some(Ann::Arrow(_, b))) => *b = pos,
                        (Phase::Annotating, Some(Ann::Pen(pts))) => pts.push(pos),
                        _ => {}
                    }
                    changed = true;
                }
                PointerEventKind::Press { button: BTN_LEFT, .. } => {
                    match ui.phase {
                        Phase::Idle => {
                            ui.phase = Phase::Selecting;
                            ui.start = pos;
                            ui.dragged = false;
                        }
                        Phase::Annotating => match ui.tool {
                            Some(Tool::Marker) => ui.anns.push(Ann::Marker(pos, ui.next_marker())),
                            Some(Tool::Arrow) => ui.drawing = Some(Ann::Arrow(pos, pos)),
                            Some(Tool::Pen) => ui.drawing = Some(Ann::Pen(vec![pos])),
                            None => {}
                        },
                        Phase::Selecting => {}
                    }
                    changed = true;
                }
                PointerEventKind::Press { button: BTN_RIGHT, .. } if ui.phase != Phase::Annotating => {
                    ui.outcome = Some(Outcome::Cancel);
                }
                PointerEventKind::Release { button: BTN_LEFT, .. } => {
                    match ui.phase {
                        Phase::Selecting => self.finish_selection(),
                        Phase::Annotating => {
                            if let Some(a) = self.ui.drawing.take() {
                                self.ui.anns.push(a);
                            }
                        }
                        Phase::Idle => {}
                    }
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.redraw(qh);
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(App);

// ---------------------------------------------------------------- capture & output

/// Frame rectangles of visible windows on the current desktop, topmost first, in logical
/// compositor coordinates. Empty when KWin scripting is unavailable; snapping then falls back to whole outputs.
fn kwin_windows() -> Vec<Rect> {
    struct Receiver(std::sync::mpsc::Sender<String>);
    #[zbus::interface(name = "org.snip.Windows")]
    impl Receiver {
        #[zbus(name = "report")]
        fn report(&self, list: String) {
            let _ = self.0.send(list);
        }
    }
    let pid = std::process::id();
    let service = format!("org.snip.Snip{pid}");
    let name = format!("snip-{pid}");
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{dir}/{name}.js");
    let script = format!(
        r#"const cur = workspace.currentDesktop;
const list = workspace.stackingOrder.filter(w =>
    (w.normalWindow || w.dialog || w.dock) && !w.minimized && !w.hidden &&
    (w.onAllDesktops || w.desktops.includes(cur))
).reverse().map(w => {{ const g = w.frameGeometry; return [g.x, g.y, g.width, g.height].join(","); }});
callDBus("{service}", "/", "org.snip.Windows", "report", list.join(";"));"#
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let run = || -> zbus::Result<String> {
        let conn = zbus::blocking::connection::Builder::session()?.name(service.as_str())?.serve_at("/", Receiver(tx))?.build()?;
        std::fs::write(&path, &script).map_err(|e| zbus::Error::Failure(e.to_string()))?;
        let reply = conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), "loadScript", &(path.as_str(), name.as_str()));
        let id: i32 = reply?.body().deserialize()?;
        let ran = conn.call_method(Some("org.kde.KWin"), format!("/Scripting/Script{id}").as_str(), Some("org.kde.kwin.Script"), "run", &());
        let got = ran.map(|_| rx.recv_timeout(std::time::Duration::from_millis(1500)));
        let _ = conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), "unloadScript", &(name.as_str(),));
        got?.map_err(|_| zbus::Error::Failure("kwin script did not report".into()))
    };
    let result = run();
    let _ = std::fs::remove_file(&path);
    match result {
        Ok(list) => list
            .split(';')
            .filter_map(|r| {
                let v: Vec<f32> = r.split(',').filter_map(|n| n.parse().ok()).collect();
                let (x, y) = (v.first()?.round(), v.get(1)?.round());
                Some(Rect { x, y, w: (v.first()? + v.get(2)?).round() - x, h: (v.get(1)? + v.get(3)?).round() - y })
            })
            .collect(),
        Err(e) => {
            eprintln!("snip: window list unavailable: {e}");
            Vec::new()
        }
    }
}

fn capture() -> RgbaImage {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{dir}/snip-{}.png", std::process::id());
    let ok = Command::new("spectacle")
        .args(["-b", "-n", "-f", "-o", &path])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let img = image::open(&path);
    let _ = std::fs::remove_file(&path);
    match (ok, img) {
        (true, Ok(i)) => i.to_rgba8(),
        _ => die("screen capture with `spectacle -b -f` failed"),
    }
}

/// Lets the shell / hotkey daemon move on while this process keeps serving the clipboard.
fn detach() {
    unsafe {
        match libc::fork() {
            -1 => {}
            0 => {
                libc::setsid();
            }
            _ => std::process::exit(0),
        }
    }
}

fn copy_to_clipboard(data: Vec<u8>, mime: wl_clipboard_rs::copy::MimeType) {
    use wl_clipboard_rs::copy::{Options, Source};
    detach();
    let mut o = Options::new();
    o.foreground(true); // serve until another client owns the clipboard
    if let Err(e) = o.copy(Source::Bytes(data.into()), mime) {
        die(&format!("clipboard: {e}"));
    }
}

fn png_bytes(img: &image::DynamicImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap_or_else(|e| die(&format!("png encode: {e}")));
    out.into_inner()
}

fn ocr(crop: &RgbaImage) -> String {
    let mut gray: GrayImage = image::DynamicImage::ImageRgba8(crop.clone()).to_luma8();
    // tesseract wants dark text on a light background at roughly 300 dpi
    let mean = gray.pixels().map(|p| p.0[0] as u64).sum::<u64>() / (gray.len() as u64).max(1);
    if mean < 128 {
        imageops::invert(&mut gray);
    }
    let k = if gray.height() < 600 { 3 } else { 2 };
    let big = imageops::resize(&gray, gray.width() * k, gray.height() * k, imageops::FilterType::CatmullRom);
    let png = png_bytes(&image::DynamicImage::ImageLuma8(big));

    let mut cmd = match std::env::var("SNIP_OCR_CMD") {
        Ok(c) if !c.trim().is_empty() => {
            let mut c2 = Command::new("sh");
            c2.args(["-c", &c]);
            c2
        }
        _ => {
            let lang = std::env::var("SNIP_OCR_LANG").unwrap_or_else(|_| "eng".into());
            let psm = std::env::var("SNIP_OCR_PSM").unwrap_or_else(|_| "6".into());
            let mut c = Command::new("tesseract");
            c.args(["stdin", "stdout", "-l", &lang, "--psm", &psm, "-c", "preserve_interword_spaces=1"]);
            c
        }
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| die(&format!("ocr: {e}")));
    let mut stdin = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&png);
    });
    let out = child.wait_with_output().unwrap_or_else(|e| die(&format!("ocr: {e}")));
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    lines.join("\n").trim_matches('\n').replace('\u{c}', "")
}

fn notify(summary: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["-a", "snip", "-i", "edit-copy", "-t", "3000", "-h", "string:x-kde-origin-name:snip", summary, body])
        .stdout(Stdio::null())
        .spawn();
}

fn save_dir() -> std::path::PathBuf {
    let pics = Command::new("xdg-user-dir")
        .arg("PICTURES")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/Pictures", std::env::var("HOME").unwrap_or_default()));
    std::path::Path::new(&pics).join("Screenshots")
}

fn main() {
    let purpose = match std::env::args().nth(1).as_deref() {
        None | Some("shot") => Purpose::Shot,
        Some("ocr") => Purpose::Ocr,
        Some("windows") => {
            // diagnostic: what click-to-snap would offer, topmost first
            for r in kwin_windows() {
                println!("{} {} {}x{}", r.x, r.y, r.w, r.h);
            }
            return;
        }
        _ => {
            eprintln!("usage: snip [shot|ocr|windows]");
            std::process::exit(2)
        }
    };
    let font_thread = std::thread::spawn(load_font);
    let windows_thread = std::thread::spawn(kwin_windows);
    let img = capture();

    let conn = Connection::connect_to_env().unwrap_or_else(|_| die("no wayland display"));
    let (globals, mut queue) = registry_queue_init(&conn).unwrap_or_else(|_| die("wayland registry"));
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).unwrap_or_else(|_| die("no wl_compositor"));
    let layer_shell = LayerShell::bind(&globals, &qh).unwrap_or_else(|_| die("compositor lacks wlr-layer-shell"));
    let shm = Shm::bind(&globals, &qh).unwrap_or_else(|_| die("no wl_shm"));
    let mut app = App {
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        shm,
        cursor_shape: CursorShapeManager::bind(&globals, &qh).ok(),
        shape_device: None,
        keyboard: None,
        pointer: None,
        screens: Vec::new(),
        font: None,
        ui: Ui {
            purpose,
            phase: Phase::Idle,
            start: (0.0, 0.0),
            cur: (-1.0, -1.0),
            sel: None,
            dragged: false,
            windows: Vec::new(),
            outputs: Vec::new(),
            tool: None,
            anns: Vec::new(),
            drawing: None,
            ctrl: false,
            outcome: None,
        },
    };
    queue.roundtrip(&mut app).unwrap_or_else(|_| die("wayland roundtrip"));
    queue.roundtrip(&mut app).unwrap_or_else(|_| die("wayland roundtrip"));

    // Output geometry in logical (compositor) coordinates; the capture spans their bounding box.
    let outs: Vec<(wl_output::WlOutput, i32, i32, i32, i32)> = app
        .outputs
        .outputs()
        .filter_map(|o| {
            let info = app.outputs.info(&o)?;
            let (x, y) = info.logical_position.unwrap_or(info.location);
            let (w, h) = info.logical_size.or_else(|| info.modes.iter().find(|m| m.current).map(|m| m.dimensions))?;
            Some((o, x, y, w, h))
        })
        .collect();
    if outs.is_empty() {
        die("no outputs");
    }
    let minx = outs.iter().map(|o| o.1).min().unwrap();
    let miny = outs.iter().map(|o| o.2).min().unwrap();
    let maxx = outs.iter().map(|o| o.1 + o.3).max().unwrap();
    let scale = img.width() as f32 / (maxx - minx) as f32; // capture pixels per logical pixel
    app.ui.outputs = outs.iter().map(|o| Rect { x: o.1 as f32, y: o.2 as f32, w: o.3 as f32, h: o.4 as f32 }).collect();
    // clip windows to the captured area (they may hang off an edge)
    let clip = |r: Rect| app.ui.outputs.iter().filter_map(|o| r.intersect(o)).reduce(|a, b| Rect::span((a.x.min(b.x), a.y.min(b.y)), ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h))));
    app.ui.windows = windows_thread.join().unwrap_or_default().into_iter().filter_map(clip).collect();

    for (o, x, y, w, h) in outs {
        let (w, h) = (w as u32, h as u32);
        let mut bright = vec![0u8; (w * h * 4) as usize];
        for ly in 0..h {
            let iy = (((y - miny) as f32 + ly as f32 + 0.5) * scale) as u32;
            for lx in 0..w {
                let ix = (((x - minx) as f32 + lx as f32 + 0.5) * scale) as u32;
                let p = img.get_pixel_checked(ix, iy).map_or([0, 0, 0, 255], |p| p.0);
                let i = ((ly * w + lx) * 4) as usize;
                bright[i..i + 4].copy_from_slice(&[p[2], p[1], p[0], 255]);
            }
        }
        let dim = bright.iter().enumerate().map(|(i, &v)| if i % 4 == 3 { 255 } else { ((v as u32 * DIM) >> 8) as u8 }).collect();
        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("snip"), Some(&o));
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(w, h);
        layer.commit();
        let pool = SlotPool::new((w * h * 4) as usize * 2, &app.shm).unwrap_or_else(|_| die("shm pool"));
        app.screens.push(Screen { layer, x, y, w, h, bright, dim, pool, configured: false, frame_pending: false, dirty: true });
    }
    app.font = font_thread.join().ok().flatten();

    while app.ui.outcome.is_none() {
        if queue.blocking_dispatch(&mut app).is_err() {
            die("wayland connection lost");
        }
    }
    let anns = std::mem::take(&mut app.ui.anns);
    let (outcome, sel) = (app.ui.outcome.unwrap(), app.ui.sel);
    let font = app.font.take();
    app.screens.clear(); // unmaps the overlays
    let _ = queue.roundtrip(&mut app);
    drop(app);
    drop(queue);
    drop(conn);

    let (Some(r), Outcome::Copy | Outcome::Save) = (sel, outcome) else { return };
    let ix = ((r.x - minx as f32) * scale).round().max(0.0) as u32;
    let iy = ((r.y - miny as f32) * scale).round().max(0.0) as u32;
    let iw = ((r.w * scale).round() as u32).min(img.width().saturating_sub(ix)).max(1);
    let ih = ((r.h * scale).round() as u32).min(img.height().saturating_sub(iy)).max(1);
    let crop = imageops::crop_imm(&img, ix, iy, iw, ih).to_image();

    if purpose == Purpose::Ocr {
        let text = ocr(&crop);
        if text.trim().is_empty() {
            notify("No text found", "");
            return;
        }
        let preview: String = text.chars().take(200).collect();
        notify("Text copied", &preview);
        copy_to_clipboard(text.into_bytes(), wl_clipboard_rs::copy::MimeType::Text);
        return;
    }

    let mut out = crop;
    if !anns.is_empty() {
        let (w, h) = out.dimensions();
        let mut pm = Pixmap::from_vec(out.into_raw(), tiny_skia::IntSize::from_wh(w, h).unwrap()).unwrap();
        let tf = Transform::from_translate(-r.x, -r.y).post_scale(scale, scale);
        for a in &anns {
            draw_ann(&mut pm.as_mut(), a, tf, false, font.as_ref());
        }
        // annotations are opaque over an opaque image, so premultiplied == straight alpha here
        out = RgbaImage::from_raw(w, h, pm.take()).unwrap();
    }
    let png = png_bytes(&image::DynamicImage::ImageRgba8(out));
    if outcome == Outcome::Save {
        let dir = save_dir();
        let stamp = Command::new("date").arg("+%Y-%m-%d_%H-%M-%S").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let path = dir.join(format!("snip_{}.png", stamp.unwrap_or_default()));
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &png)) {
            Ok(()) => notify("Screenshot saved and copied", &path.display().to_string()),
            Err(e) => notify("Saving failed", &e.to_string()),
        }
    }
    copy_to_clipboard(png, wl_clipboard_rs::copy::MimeType::Specific("image/png".into()));
}
