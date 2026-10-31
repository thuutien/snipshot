//! Text annotations: one font used both on screen (through egui) and when
//! burning text into the saved image (through ab_glyph), so they match.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use eframe::egui::{self, Color32, FontFamily, FontId, Painter, Pos2, Rect, vec2};
use image::RgbaImage;

const FONT_NAME: &str = "snip-text";
const FONT_DATA: &[u8] = epaint_default_fonts::UBUNTU_LIGHT;

/// The font is a light weight, so text is drawn twice with this horizontal
/// offset (as a fraction of the size) to make it bolder and easier to read.
const BOLD: f32 = 0.035;

/// A text label, in image pixel coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct TextMark {
    /// Top-left corner.
    pub pos: Pos2,
    pub text: String,
    pub color: Color32,
    /// Font size (em) in image pixels.
    pub size: f32,
}

pub fn register_font(ctx: &egui::Context) {
    ctx.add_font(egui::epaint::text::FontInsert {
        name: FONT_NAME.to_owned(),
        data: egui::FontData::from_static(FONT_DATA),
        families: vec![egui::epaint::text::InsertFontFamily {
            family: family(),
            priority: egui::epaint::text::FontPriority::Highest,
        }],
    });
}

fn family() -> FontFamily {
    FontFamily::Name(FONT_NAME.into())
}

pub fn font_id(size: f32) -> FontId {
    FontId::new(size, family())
}

impl TextMark {
    /// Bounds in image pixels.
    pub fn bounds(&self, painter: &Painter) -> Rect {
        let text = if self.text.is_empty() { " " } else { &self.text };
        let galley = painter.layout_no_wrap(text.to_owned(), font_id(self.size), self.color);
        Rect::from_min_size(self.pos, galley.size() + vec2(self.size * BOLD, 0.0))
    }

    /// Paints the label, with `z` screen points per image pixel.
    pub fn paint(&self, painter: &Painter, to_screen: impl Fn(Pos2) -> Pos2, z: f32) {
        let font = font_id(self.size * z);
        let at = to_screen(self.pos);
        for dx in [0.0, self.size * BOLD * z] {
            painter.text(at + vec2(dx, 0.0), egui::Align2::LEFT_TOP, &self.text, font.clone(), self.color);
        }
    }
}

/// Burns the label into the image.
pub fn draw_text(image: &mut RgbaImage, mark: &TextMark) {
    let Ok(font) = FontRef::try_from_slice(FONT_DATA) else { return };
    // egui sizes fonts by em; ab_glyph's PxScale is the ascent-to-descent height.
    let units_per_em = font.units_per_em().unwrap_or(1000.0);
    let scale = PxScale::from(mark.size * font.height_unscaled() / units_per_em);
    let scaled = font.as_scaled(scale);
    let line_height = scaled.height() + scaled.line_gap();
    let [r, g, b, a] = mark.color.to_array();
    let (w, h) = image.dimensions();

    for offset in [0.0, mark.size * BOLD] {
        for (row, line) in mark.text.lines().enumerate() {
            let baseline = mark.pos.y + scaled.ascent() + row as f32 * line_height;
            let mut x = mark.pos.x + offset;
            let mut previous = None;
            for ch in line.chars() {
                let id = font.glyph_id(ch);
                if let Some(prev) = previous {
                    x += scaled.kern(prev, id);
                }
                previous = Some(id);
                let glyph = id.with_scale_and_position(scale, point(x, baseline));
                x += scaled.h_advance(id);
                let Some(outline) = font.outline_glyph(glyph) else { continue };
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                        return;
                    }
                    let alpha = coverage.clamp(0.0, 1.0) * a as f32 / 255.0;
                    let dst = image.get_pixel_mut(px as u32, py as u32);
                    for (d, s) in dst.0.iter_mut().zip([r, g, b]) {
                        *d = (*d as f32 * (1.0 - alpha) + s as f32 * alpha).round() as u8;
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;
    use image::Rgba;

    #[test]
    fn text_is_burned_in_below_and_right_of_its_corner() {
        let mut image = RgbaImage::from_pixel(200, 80, Rgba([255, 255, 255, 255]));
        let mark = TextMark { pos: pos2(20.0, 10.0), text: "Hi\nyo".into(), color: Color32::RED, size: 24.0 };
        draw_text(&mut image, &mark);
        let inked: Vec<(u32, u32)> = image
            .enumerate_pixels()
            .filter(|(_, _, p)| p.0[1] < 128)
            .map(|(x, y, _)| (x, y))
            .collect();
        assert!(inked.len() > 50);
        assert!(inked.iter().all(|&(x, y)| x >= 20 && y >= 10));
        // The second line lands roughly one line height lower.
        assert!(inked.iter().any(|&(_, y)| y > 40));
    }
}
