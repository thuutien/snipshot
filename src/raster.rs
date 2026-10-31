//! Burns ink strokes into the snip when it is copied or saved.

use eframe::egui::{Pos2, pos2};
use image::RgbaImage;

use crate::editor::{HIGHLIGHTER_OPACITY, Ink, InkStroke};

pub fn draw_stroke(image: &mut RgbaImage, stroke: &InkStroke) {
    let (w, h) = image.dimensions();
    let radius = stroke.width / 2.0;
    let pad = radius + 1.0;

    let (mut min, mut max) = (stroke.points[0], stroke.points[0]);
    for p in &stroke.points {
        min = min.min(*p);
        max = max.max(*p);
    }
    let x0 = (min.x - pad).floor().max(0.0) as u32;
    let y0 = (min.y - pad).floor().max(0.0) as u32;
    let x1 = ((max.x + pad).ceil() as u32).min(w);
    let y1 = ((max.y + pad).ceil() as u32).min(h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    // Coverage is accumulated with max() so overlapping segments don't
    // darken a translucent highlighter stroke.
    let bw = (x1 - x0) as usize;
    let mut coverage = vec![0f32; bw * (y1 - y0) as usize];
    let first = [stroke.points[0], stroke.points[0]];
    let segments = if stroke.points.len() == 1 { &first[..] } else { &stroke.points[..] };
    for seg in segments.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let sx0 = ((a.x.min(b.x) - pad).floor().max(x0 as f32)) as u32;
        let sy0 = ((a.y.min(b.y) - pad).floor().max(y0 as f32)) as u32;
        let sx1 = ((a.x.max(b.x) + pad).ceil() as u32).min(x1);
        let sy1 = ((a.y.max(b.y) + pad).ceil() as u32).min(y1);
        for y in sy0..sy1 {
            for x in sx0..sx1 {
                let d = distance_to_segment(pos2(x as f32 + 0.5, y as f32 + 0.5), a, b);
                let c = (radius + 0.5 - d).clamp(0.0, 1.0);
                let cell = &mut coverage[(y - y0) as usize * bw + (x - x0) as usize];
                *cell = cell.max(c);
            }
        }
    }

    let opacity = match stroke.ink {
        Ink::Pen | Ink::Shape => 1.0,
        Ink::Highlighter => HIGHLIGHTER_OPACITY,
    };
    let [r, g, b, _] = stroke.color.to_array();
    for y in y0..y1 {
        for x in x0..x1 {
            let a = coverage[(y - y0) as usize * bw + (x - x0) as usize] * opacity;
            if a <= 0.0 {
                continue;
            }
            let px = image.get_pixel_mut(x, y);
            for (dst, src) in px.0.iter_mut().zip([r, g, b]) {
                *dst = (*dst as f32 * (1.0 - a) + src as f32 * a).round() as u8;
            }
            px.0[3] = 255;
        }
    }
}

pub fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 == 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;
    use image::Rgba;

    fn stroke(ink: Ink, points: Vec<Pos2>) -> InkStroke {
        InkStroke { ink, color: Color32::from_rgb(255, 0, 0), width: 4.0, points }
    }

    #[test]
    fn pen_paints_solid_line() {
        let mut image = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
        draw_stroke(&mut image, &stroke(Ink::Pen, vec![pos2(2.0, 10.0), pos2(18.0, 10.0)]));
        assert_eq!(image.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(10, 2).0, [255, 255, 255, 255]);
    }

    #[test]
    fn highlighter_blends_once_where_segments_overlap() {
        let mut image = RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255]));
        let back_and_forth = vec![pos2(2.0, 10.0), pos2(18.0, 10.0), pos2(2.0, 10.0)];
        draw_stroke(&mut image, &stroke(Ink::Highlighter, back_and_forth));
        let [r, _, b, _] = image.get_pixel(10, 10).0;
        assert_eq!(r, (255.0 * HIGHLIGHTER_OPACITY).round() as u8);
        assert_eq!(b, (255.0 * (1.0 - HIGHLIGHTER_OPACITY)).round() as u8);
    }

    #[test]
    fn single_point_makes_a_dot_and_offscreen_is_ignored() {
        let mut image = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
        draw_stroke(&mut image, &stroke(Ink::Pen, vec![pos2(5.0, 5.0)]));
        assert_eq!(image.get_pixel(4, 4).0, [255, 0, 0, 255]);
        draw_stroke(&mut image, &stroke(Ink::Pen, vec![pos2(-50.0, -50.0)]));
    }
}
