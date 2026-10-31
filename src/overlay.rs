//! The full-screen snipping overlay: a frozen, dimmed screenshot with a mode
//! toolbar at the top, where the user picks the area to snip.

use eframe::egui::{
    self, Align2, Color32, ColorImage, CursorIcon, FontId, Id, Key, Painter, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use image::{Rgba, RgbaImage};

use crate::app::SnipMode;
use crate::capture::{Capture, IRect};
use crate::icons::{self, Icon};

pub const ACCENT: Color32 = Color32::from_rgb(0, 120, 212);
const DIM: Color32 = Color32::from_black_alpha(140);

pub enum Outcome {
    Pending,
    Cancel,
    Done(RgbaImage),
}

pub struct Overlay {
    capture: Capture,
    texture: TextureHandle,
    pub mode: SnipMode,
    /// Rectangle drag, start and current point in image pixels.
    drag: Option<(Pos2, Pos2)>,
    /// Freeform outline in image pixels.
    lasso: Vec<Pos2>,
}

pub fn to_color_image(image: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw())
}

impl Overlay {
    pub fn new(ctx: &egui::Context, capture: Capture, mode: SnipMode) -> Self {
        let texture = ctx.load_texture("snip-screen", to_color_image(&capture.image), TextureOptions::LINEAR);
        Self { capture, texture, mode, drag: None, lasso: Vec::new() }
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Outcome {
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            return Outcome::Cancel;
        }

        let mut outcome = Outcome::Pending;
        let mut picked = None;
        egui::Area::new(Id::unique("snip-toolbar"))
            .anchor(Align2::CENTER_TOP, vec2(0.0, 18.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).corner_radius(8).inner_margin(6).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for mode in SnipMode::ALL {
                            if icons::button(ui, mode.icon(), self.mode == mode, mode.label()).clicked() {
                                picked = Some(mode);
                            }
                        }
                        ui.separator();
                        if icons::button(ui, Icon::Close, false, "Cancel (Esc)").clicked() {
                            outcome = Outcome::Cancel;
                        }
                    });
                });
            });
        if let Some(mode) = picked {
            self.mode = mode;
            self.drag = None;
            self.lasso.clear();
            if mode == SnipMode::Fullscreen {
                return Outcome::Done(self.capture.image.clone());
            }
        }
        if matches!(outcome, Outcome::Cancel) {
            return outcome;
        }

        let screen = ui.max_rect();
        let (w, h) = self.capture.image.dimensions();
        let (sx, sy) = (w as f32 / screen.width(), h as f32 / screen.height());
        let to_px = |p: Pos2| pos2((p.x - screen.min.x) * sx, (p.y - screen.min.y) * sy);
        let to_pt = |p: Pos2| pos2(screen.min.x + p.x / sx, screen.min.y + p.y / sy);

        let response = ui.interact(screen, Id::unique("snip-surface"), Sense::click_and_drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        let press_origin = ui.input(|i| i.pointer.press_origin());
        let painter = ui.painter();
        painter.image(self.texture.id(), screen, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);

        let window_snip = self.mode == SnipMode::Window && !self.capture.windows.is_empty();
        if window_snip {
            let hovered = response
                .hover_pos()
                .and_then(|p| self.capture.windows.iter().find(|r| r.contains(to_px(p))).copied());
            match hovered {
                Some(r) => {
                    let hole = Rect::from_min_max(
                        to_pt(pos2(r.x as f32, r.y as f32)),
                        to_pt(pos2(r.right() as f32, r.bottom() as f32)),
                    );
                    dim_around(painter, screen, hole);
                    painter.rect_stroke(hole, 0.0, Stroke::new(3.0, ACCENT), StrokeKind::Inside);
                    if response.clicked() {
                        outcome = Outcome::Done(crop(&self.capture.image, r));
                    }
                }
                None => {
                    painter.rect_filled(screen, 0.0, DIM);
                }
            }
        } else if self.mode == SnipMode::Freeform {
            if response.drag_started() {
                self.lasso.clear();
                self.lasso.extend(press_origin.map(to_px));
            }
            if response.dragged()
                && let Some(p) = response.interact_pointer_pos().map(to_px)
                && self.lasso.last().is_none_or(|last| last.distance(p) >= 1.0)
            {
                self.lasso.push(p);
            }
            painter.rect_filled(screen, 0.0, DIM);
            if self.lasso.len() > 1 {
                let points: Vec<Pos2> = self.lasso.iter().map(|&p| to_pt(p)).collect();
                painter.add(Shape::line(points.clone(), Stroke::new(4.0, ACCENT)));
                painter.add(Shape::line(points, Stroke::new(1.5, Color32::WHITE)));
            }
            if response.drag_stopped() {
                if self.lasso.len() >= 3 {
                    if let Some(image) = freeform_crop(&self.capture.image, &self.lasso) {
                        outcome = Outcome::Done(image);
                    }
                }
                self.lasso.clear();
            }
        } else {
            // Rectangle snip (also used for window snips when window positions
            // are unknown, as on Wayland).
            if response.drag_started()
                && let Some(origin) = press_origin.map(to_px)
            {
                self.drag = Some((origin, origin));
            }
            if response.dragged()
                && let (Some(drag), Some(p)) = (&mut self.drag, response.interact_pointer_pos())
            {
                drag.1 = to_px(p);
            }
            match self.drag {
                Some((a, b)) => {
                    let hole = Rect::from_two_pos(to_pt(a), to_pt(b));
                    dim_around(painter, screen, hole);
                    painter.rect_stroke(hole, 0.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);
                    let r = pixel_rect(a, b, w, h);
                    size_label(painter, hole, screen, &format!("{} × {}", r.w, r.h));
                }
                None => {
                    painter.rect_filled(screen, 0.0, DIM);
                }
            }
            if response.drag_stopped()
                && let Some((a, b)) = self.drag.take()
            {
                let r = pixel_rect(a, b, w, h);
                if r.w >= 2 && r.h >= 2 {
                    outcome = Outcome::Done(crop(&self.capture.image, r));
                }
            }
        }

        outcome
    }
}

/// Dims everything on `screen` except `hole`.
fn dim_around(painter: &Painter, screen: Rect, hole: Rect) {
    let hole = hole.intersect(screen);
    let parts = [
        Rect::from_min_max(screen.min, pos2(screen.max.x, hole.min.y)),
        Rect::from_min_max(pos2(screen.min.x, hole.max.y), screen.max),
        Rect::from_min_max(pos2(screen.min.x, hole.min.y), pos2(hole.min.x, hole.max.y)),
        Rect::from_min_max(pos2(hole.max.x, hole.min.y), pos2(screen.max.x, hole.max.y)),
    ];
    for part in parts {
        if part.is_positive() {
            painter.rect_filled(part, 0.0, DIM);
        }
    }
}

fn size_label(painter: &Painter, hole: Rect, screen: Rect, text: &str) {
    let font = FontId::proportional(15.0);
    let above = hole.top() - 22.0 > screen.top();
    let (pos, anchor) = if above {
        (hole.left_top() - vec2(0.0, 6.0), Align2::LEFT_BOTTOM)
    } else {
        (hole.left_top() + vec2(6.0, 6.0), Align2::LEFT_TOP)
    };
    let galley = painter.layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let rect = anchor.anchor_size(pos, galley.size()).expand2(vec2(6.0, 3.0));
    painter.rect_filled(rect, 4.0, Color32::from_black_alpha(200));
    painter.galley(rect.min + vec2(6.0, 3.0), galley, Color32::WHITE);
}

fn pixel_rect(a: Pos2, b: Pos2, w: u32, h: u32) -> IRect {
    let x0 = a.x.min(b.x).round().clamp(0.0, w as f32) as i32;
    let y0 = a.y.min(b.y).round().clamp(0.0, h as f32) as i32;
    let x1 = a.x.max(b.x).round().clamp(0.0, w as f32) as i32;
    let y1 = a.y.max(b.y).round().clamp(0.0, h as f32) as i32;
    IRect { x: x0, y: y0, w: (x1 - x0) as u32, h: (y1 - y0) as u32 }
}

fn crop(image: &RgbaImage, r: IRect) -> RgbaImage {
    image::imageops::crop_imm(image, r.x as u32, r.y as u32, r.w, r.h).to_image()
}

/// Crops to the outline's bounding box and paints everything outside the
/// outline white, like the Windows freeform snip.
fn freeform_crop(image: &RgbaImage, outline: &[Pos2]) -> Option<RgbaImage> {
    let (w, h) = image.dimensions();
    let (mut min, mut max) = (outline[0], outline[0]);
    for p in outline {
        min = min.min(*p);
        max = max.max(*p);
    }
    let bounds = pixel_rect(min, max, w, h);
    if bounds.w < 2 || bounds.h < 2 {
        return None;
    }
    let source = crop(image, bounds);
    let mut out = RgbaImage::from_pixel(bounds.w, bounds.h, Rgba([255, 255, 255, 255]));

    // Scanline fill with the even-odd rule.
    let mut crossings = Vec::new();
    for row in 0..bounds.h {
        let y = bounds.y as f32 + row as f32 + 0.5;
        crossings.clear();
        for (i, a) in outline.iter().enumerate() {
            let b = outline[(i + 1) % outline.len()];
            if (a.y <= y) != (b.y <= y) {
                crossings.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
            }
        }
        crossings.sort_by(f32::total_cmp);
        for span in crossings.chunks_exact(2) {
            let from = (span[0] - bounds.x as f32 - 0.5).ceil().max(0.0) as u32;
            let to = (span[1] - bounds.x as f32 - 0.5).floor().min(bounds.w as f32 - 1.0);
            if to < 0.0 {
                continue;
            }
            for col in from..=to as u32 {
                out.put_pixel(col, row, *source.get_pixel(col, row));
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_rect_normalizes_and_clamps() {
        let r = pixel_rect(pos2(50.5, 40.2), pos2(-10.0, 10.0), 100, 100);
        assert_eq!(r, IRect { x: 0, y: 10, w: 51, h: 30 });
    }

    #[test]
    fn freeform_keeps_inside_and_whitens_outside() {
        let image = RgbaImage::from_pixel(100, 100, Rgba([10, 20, 30, 255]));
        // A triangle: top-left corner of the bounding box is outside it.
        let outline = [pos2(10.0, 10.0), pos2(90.0, 10.0), pos2(90.0, 90.0)];
        let out = freeform_crop(&image, &outline).unwrap();
        assert_eq!(out.dimensions(), (80, 80));
        assert_eq!(out.get_pixel(75, 5).0, [10, 20, 30, 255]);
        assert_eq!(out.get_pixel(5, 75).0, [255, 255, 255, 255]);
    }
}
