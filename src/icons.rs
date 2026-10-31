//! Toolbar icons, drawn with the painter so they look the same everywhere
//! (no icon font or image assets needed).

use std::f32::consts::{PI, TAU};

use eframe::egui::{Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Rectangle,
    Window,
    Fullscreen,
    Freeform,
    Pen,
    Highlighter,
    Eraser,
    Shapes,
    Ellipse,
    Line,
    Arrow,
    Text,
    Crop,
    Undo,
    Redo,
    ZoomIn,
    ZoomOut,
    Copy,
    Save,
    Close,
    Check,
}

/// A square icon button with a tooltip.
pub fn button(ui: &mut Ui, icon: Icon, selected: bool, tooltip: &str) -> Response {
    button_ex(ui, icon, selected, true, None, tooltip)
}

/// Like [`button`], optionally disabled and with an accent color (used to
/// show the current pen color).
pub fn button_ex(
    ui: &mut Ui,
    icon: Icon,
    selected: bool,
    enabled: bool,
    accent: Option<Color32>,
    tooltip: &str,
) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 30.0), sense);
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let bg = if selected {
            visuals.selection.bg_fill.gamma_multiply(0.55)
        } else if enabled && response.hovered() {
            visuals.widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 6.0, bg);
        let color = if enabled { visuals.strong_text_color() } else { visuals.weak_text_color() };
        paint(ui.painter(), icon, Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), color, accent);
    }
    response.on_hover_text(tooltip)
}

pub fn paint(painter: &Painter, icon: Icon, r: Rect, color: Color32, accent: Option<Color32>) {
    let p = |x: f32, y: f32| pos2(r.left() + x * r.width(), r.top() + y * r.height());
    let s = Stroke::new(1.6, color);
    let line = |pts: &[(f32, f32)], stroke: Stroke| {
        painter.add(Shape::line(pts.iter().map(|&(x, y)| p(x, y)).collect(), stroke));
    };
    let arc = |cx: f32, cy: f32, radius: f32, from: f32, to: f32| -> Vec<Pos2> {
        (0..=16)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 16.0;
                p(cx + radius * a.cos(), cy + radius * a.sin())
            })
            .collect()
    };

    match icon {
        Icon::Rectangle => {
            painter.rect_stroke(Rect::from_min_max(p(0.08, 0.2), p(0.92, 0.8)), 2.0, s, StrokeKind::Middle);
        }
        Icon::Window => {
            painter.rect_stroke(Rect::from_min_max(p(0.06, 0.14), p(0.94, 0.86)), 2.0, s, StrokeKind::Middle);
            line(&[(0.06, 0.36), (0.94, 0.36)], s);
        }
        Icon::Fullscreen => {
            for (x, y, dx, dy) in [(0.08, 0.12, 1.0, 1.0), (0.92, 0.12, -1.0, 1.0), (0.08, 0.88, 1.0, -1.0), (0.92, 0.88, -1.0, -1.0)] {
                line(&[(x, y + 0.3 * dy), (x, y), (x + 0.3 * dx, y)], s);
            }
        }
        Icon::Freeform => {
            let pts = (0..32)
                .map(|i| {
                    let t = i as f32 / 32.0 * TAU;
                    let radius = 0.36 + 0.08 * (3.0 * t).sin();
                    p(0.5 + radius * t.cos(), 0.5 + radius * 0.85 * t.sin())
                })
                .collect();
            painter.add(Shape::closed_line(pts, s));
        }
        Icon::Pen => {
            line(&[(0.3, 0.7), (0.85, 0.15)], Stroke::new(3.2, color));
            line(&[(0.12, 0.88), (0.3, 0.7)], Stroke::new(2.0, accent.unwrap_or(color)));
        }
        Icon::Highlighter => {
            line(&[(0.38, 0.62), (0.82, 0.18)], Stroke::new(5.0, color));
            line(&[(0.2, 0.8), (0.38, 0.62)], Stroke::new(5.0, accent.unwrap_or(color)));
            line(&[(0.05, 0.95), (0.6, 0.95)], Stroke::new(2.0, accent.unwrap_or(color)));
        }
        Icon::Eraser => {
            // A tilted eraser block with a solid rubber tip resting on a line.
            let center = vec2(0.56, 0.42);
            let along = vec2(1.0, -1.0).normalized();
            let across = vec2(1.0, 1.0).normalized();
            let (half_len, half_wid, tip) = (0.4, 0.19, 0.32);
            let at = |a: f32, b: f32| {
                let v = center + along * a + across * b;
                p(v.x, v.y)
            };
            let tip_end = -half_len + tip;
            painter.add(Shape::convex_polygon(
                vec![at(-half_len, -half_wid), at(tip_end, -half_wid), at(tip_end, half_wid), at(-half_len, half_wid)],
                color,
                Stroke::NONE,
            ));
            painter.add(Shape::closed_line(
                vec![at(-half_len, -half_wid), at(half_len, -half_wid), at(half_len, half_wid), at(-half_len, half_wid)],
                s,
            ));
            line(&[(0.36, 0.96), (0.96, 0.96)], s);
        }
        Icon::Shapes => {
            painter.rect_stroke(Rect::from_min_max(p(0.04, 0.04), p(0.58, 0.58)), 1.5, s, StrokeKind::Middle);
            painter.circle_stroke(p(0.64, 0.64), r.width() * 0.32, Stroke::new(1.6, accent.unwrap_or(color)));
        }
        Icon::Ellipse => {
            let pts = (0..32)
                .map(|i| {
                    let t = i as f32 / 32.0 * TAU;
                    p(0.5 + 0.44 * t.cos(), 0.5 + 0.32 * t.sin())
                })
                .collect();
            painter.add(Shape::closed_line(pts, s));
        }
        Icon::Line => {
            line(&[(0.1, 0.9), (0.9, 0.1)], s);
        }
        Icon::Arrow => {
            line(&[(0.1, 0.9), (0.88, 0.12)], s);
            line(&[(0.46, 0.12), (0.88, 0.12), (0.88, 0.54)], s);
        }
        Icon::Text => {
            line(&[(0.12, 0.24), (0.12, 0.1), (0.88, 0.1), (0.88, 0.24)], s);
            line(&[(0.5, 0.1), (0.5, 0.9)], Stroke::new(2.0, color));
            line(&[(0.32, 0.9), (0.68, 0.9)], s);
            line(&[(0.1, 0.98), (0.9, 0.98)], Stroke::new(2.0, accent.unwrap_or(color)));
        }
        Icon::Crop => {
            line(&[(0.28, 0.02), (0.28, 0.72), (0.98, 0.72)], s);
            line(&[(0.02, 0.28), (0.72, 0.28), (0.72, 0.98)], s);
        }
        Icon::Undo | Icon::Redo => {
            let mut pts = arc(0.52, 0.6, 0.34, PI, 2.0 * PI + 0.5);
            if icon == Icon::Redo {
                for pt in &mut pts {
                    pt.x = r.left() + r.right() - pt.x;
                }
            }
            let tip = pts[0];
            painter.add(Shape::line(pts, s));
            let w = r.width() * 0.16;
            painter.line_segment([tip, tip + vec2(-w, -w)], s);
            painter.line_segment([tip, tip + vec2(w, -w)], s);
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            painter.circle_stroke(p(0.42, 0.42), r.width() * 0.3, s);
            line(&[(0.64, 0.64), (0.92, 0.92)], Stroke::new(2.2, color));
            line(&[(0.28, 0.42), (0.56, 0.42)], s);
            if icon == Icon::ZoomIn {
                line(&[(0.42, 0.28), (0.42, 0.56)], s);
            }
        }
        Icon::Copy => {
            painter.rect_stroke(Rect::from_min_max(p(0.32, 0.06), p(0.92, 0.7)), 2.0, s, StrokeKind::Middle);
            painter.rect_stroke(Rect::from_min_max(p(0.08, 0.3), p(0.68, 0.94)), 2.0, s, StrokeKind::Middle);
        }
        Icon::Save => {
            painter.rect_stroke(Rect::from_min_max(p(0.1, 0.1), p(0.9, 0.9)), 2.0, s, StrokeKind::Middle);
            painter.rect_stroke(Rect::from_min_max(p(0.3, 0.1), p(0.7, 0.36)), 0.0, s, StrokeKind::Middle);
            painter.rect_stroke(Rect::from_min_max(p(0.24, 0.56), p(0.76, 0.9)), 0.0, s, StrokeKind::Middle);
        }
        Icon::Close => {
            line(&[(0.18, 0.18), (0.82, 0.82)], s);
            line(&[(0.82, 0.18), (0.18, 0.82)], s);
        }
        Icon::Check => {
            line(&[(0.12, 0.52), (0.4, 0.8), (0.9, 0.22)], Stroke::new(2.0, color));
        }
    }
}
