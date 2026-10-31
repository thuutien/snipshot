//! The markup editor shown after a snip: pen, highlighter, eraser, crop,
//! undo/redo and zoom.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, CursorIcon, Event, Key, KeyboardShortcut, Modifiers, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};
use image::RgbaImage;

use crate::icons::{self, Icon};
use crate::overlay::{ACCENT, to_color_image};
use crate::raster::{self, distance_to_segment};
use crate::text::{self, TextMark};

pub const HIGHLIGHTER_OPACITY: f32 = 0.45;

const PEN_COLORS: [Color32; 8] = [
    Color32::from_rgb(0, 0, 0),
    Color32::from_rgb(255, 255, 255),
    Color32::from_rgb(232, 17, 35),
    Color32::from_rgb(255, 140, 0),
    Color32::from_rgb(255, 185, 0),
    Color32::from_rgb(16, 124, 16),
    Color32::from_rgb(0, 120, 212),
    Color32::from_rgb(136, 23, 152),
];

const HIGHLIGHTER_COLORS: [Color32; 6] = [
    Color32::from_rgb(255, 235, 0),
    Color32::from_rgb(0, 255, 0),
    Color32::from_rgb(0, 230, 255),
    Color32::from_rgb(255, 0, 230),
    Color32::from_rgb(255, 140, 0),
    Color32::from_rgb(255, 60, 60),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ink {
    Pen,
    Highlighter,
    /// An outline shape: drawn opaque, segment by segment.
    Shape,
}

/// One ink stroke, in image pixel coordinates.
#[derive(Clone, Debug)]
pub struct InkStroke {
    pub ink: Ink,
    pub color: Color32,
    pub width: f32,
    pub points: Vec<Pos2>,
}

impl InkStroke {
    fn display_color(&self) -> Color32 {
        match self.ink {
            Ink::Pen | Ink::Shape => self.color,
            Ink::Highlighter => {
                let [r, g, b, _] = self.color.to_array();
                Color32::from_rgba_unmultiplied(r, g, b, (HIGHLIGHTER_OPACITY * 255.0) as u8)
            }
        }
    }

    fn hit(&self, p: Pos2, radius: f32) -> bool {
        let reach = self.width / 2.0 + radius;
        match self.points.as_slice() {
            [only] => only.distance(p) <= reach,
            points => points.windows(2).any(|s| distance_to_segment(p, s[0], s[1]) <= reach),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    Pen,
    Highlighter,
    Shapes,
    Text,
    Eraser,
    Crop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ShapeKind {
    Rectangle,
    Ellipse,
    Line,
    Arrow,
}

impl ShapeKind {
    const ALL: [(ShapeKind, Icon, &'static str); 4] = [
        (Self::Rectangle, Icon::Rectangle, "Rectangle"),
        (Self::Ellipse, Icon::Ellipse, "Oval"),
        (Self::Line, Icon::Line, "Line"),
        (Self::Arrow, Icon::Arrow, "Arrow"),
    ];

    /// Outline points for a shape dragged from `a` to `b`, in image pixels.
    /// With `constrain` (Shift held), rectangles and ovals become squares and
    /// circles, and lines snap to 15° steps.
    fn points(self, a: Pos2, mut b: Pos2, width: f32, constrain: bool) -> Vec<Pos2> {
        let d = b - a;
        match self {
            Self::Rectangle | Self::Ellipse if constrain => {
                let side = d.x.abs().max(d.y.abs());
                b = a + vec2(side.copysign(d.x), side.copysign(d.y));
            }
            Self::Line | Self::Arrow if constrain && d.length() > 0.0 => {
                let step = 15f32.to_radians();
                let angle = (d.y.atan2(d.x) / step).round() * step;
                b = a + vec2(angle.cos(), angle.sin()) * d.length();
            }
            _ => {}
        }
        match self {
            Self::Rectangle => vec![a, pos2(b.x, a.y), b, pos2(a.x, b.y), a],
            Self::Ellipse => {
                let center = a + (b - a) / 2.0;
                let radius = (b - a).abs() / 2.0;
                (0..=72)
                    .map(|i| {
                        let t = i as f32 / 72.0 * std::f32::consts::TAU;
                        center + vec2(radius.x * t.cos(), radius.y * t.sin())
                    })
                    .collect()
            }
            Self::Line => vec![a, b],
            Self::Arrow => {
                let length = (b - a).length();
                if length < 1.0 {
                    return vec![a, b];
                }
                let dir = (b - a) / length;
                let head = (width * 3.5).max(12.0).min(length * 0.6);
                let wing = |angle: f32| b - egui::emath::Rot2::from_angle(angle) * dir * head;
                // Shaft, then the head as one stroke through the tip.
                vec![a, b, wing(0.5), b, wing(-0.5)]
            }
        }
    }
}

struct InkStyle {
    color: Color32,
    width: f32,
}

/// Everything undo/redo restores.
#[derive(Clone)]
struct Doc {
    strokes: Vec<InkStroke>,
    texts: Vec<TextMark>,
    /// Visible part of the snip, in image pixels.
    crop: Rect,
}

/// A text label being typed or edited.
struct EditingText {
    /// Index in `Doc::texts` when editing an existing label.
    index: Option<usize>,
    mark: TextMark,
    request_focus: bool,
}

#[derive(Clone, Copy)]
enum Grab {
    New(Pos2),
    Move,
    Edges { left: bool, right: bool, top: bool, bottom: bool },
}

pub enum Action {
    Copy,
    Save,
}

pub struct Editor {
    base: RgbaImage,
    texture: TextureHandle,
    doc: Doc,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    tool: Tool,
    tool_before_crop: Tool,
    pen: InkStyle,
    highlighter: InkStyle,
    shape: InkStyle,
    shape_kind: ShapeKind,
    /// Text color, and font size in `width`.
    text_style: InkStyle,
    editing_text: Option<EditingText>,
    moving_text: Option<usize>,
    drawing: Option<InkStroke>,
    shape_start: Option<Pos2>,
    erased_this_drag: bool,
    crop_draft: Rect,
    crop_grab: Option<Grab>,
    /// Points per image pixel; `None` fits the snip to the window.
    zoom: Option<f32>,
    fit_zoom: f32,
    status: Option<(String, Instant)>,
}

impl Editor {
    pub fn new(ctx: &egui::Context, image: RgbaImage) -> Self {
        let texture = ctx.load_texture("snip", to_color_image(&image), TextureOptions::LINEAR);
        let full = Rect::from_min_size(Pos2::ZERO, vec2(image.width() as f32, image.height() as f32));
        Self {
            base: image,
            texture,
            doc: Doc { strokes: Vec::new(), texts: Vec::new(), crop: full },
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Pen,
            tool_before_crop: Tool::Pen,
            pen: InkStyle { color: PEN_COLORS[2], width: 4.0 },
            highlighter: InkStyle { color: HIGHLIGHTER_COLORS[0], width: 18.0 },
            shape: InkStyle { color: PEN_COLORS[2], width: 4.0 },
            shape_kind: ShapeKind::Rectangle,
            text_style: InkStyle { color: PEN_COLORS[2], width: 28.0 },
            editing_text: None,
            moving_text: None,
            drawing: None,
            shape_start: None,
            erased_this_drag: false,
            crop_draft: full,
            crop_grab: None,
            zoom: None,
            fit_zoom: 1.0,
            status: None,
        }
    }

    fn full_rect(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(self.base.width() as f32, self.base.height() as f32))
    }

    /// A comfortable window size for this snip, in points.
    pub fn preferred_window_size(&self, ctx: &egui::Context) -> Vec2 {
        let ppp = ctx.pixels_per_point();
        let monitor = ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1920.0, 1080.0));
        let wanted = self.doc.crop.size() / ppp + vec2(48.0, 175.0);
        wanted.max(vec2(1000.0, 520.0)).min(monitor * 0.85)
    }

    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    /// The snip with ink burned in and the crop applied.
    pub fn render(&self) -> RgbaImage {
        let mut image = self.base.clone();
        for stroke in &self.doc.strokes {
            raster::draw_stroke(&mut image, stroke);
        }
        for mark in &self.doc.texts {
            text::draw_text(&mut image, mark);
        }
        let c = self.doc.crop;
        image::imageops::crop_imm(&image, c.min.x as u32, c.min.y as u32, c.width() as u32, c.height() as u32)
            .to_image()
    }

    // ---- history -------------------------------------------------------

    fn push_undo(&mut self) {
        self.undo.push(self.doc.clone());
        self.redo.clear();
    }

    fn undo(&mut self) {
        self.commit_text();
        if let Some(doc) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.doc, doc));
            self.after_history_change();
        }
    }

    fn redo(&mut self) {
        self.commit_text();
        if let Some(doc) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.doc, doc));
            self.after_history_change();
        }
    }

    fn after_history_change(&mut self) {
        self.crop_draft = self.doc.crop;
        self.zoom = None;
    }

    // ---- tools ---------------------------------------------------------

    fn select_tool(&mut self, tool: Tool) {
        self.commit_text();
        if tool == self.tool {
            return;
        }
        if tool == Tool::Crop {
            self.tool_before_crop = self.tool;
            self.crop_draft = self.doc.crop;
        }
        self.tool = tool;
    }

    fn apply_crop(&mut self) {
        let d = self.crop_draft;
        let r = Rect::from_min_max(d.min.round(), d.max.round()).intersect(self.full_rect());
        if r.width() >= 1.0 && r.height() >= 1.0 && r != self.doc.crop {
            self.push_undo();
            self.doc.crop = r;
            self.zoom = None;
        }
        self.tool = self.tool_before_crop;
    }

    fn cancel_crop(&mut self) {
        self.crop_draft = self.doc.crop;
        self.tool = self.tool_before_crop;
    }

    fn zoom_by(&mut self, factor: f32) {
        let z = self.zoom.unwrap_or(self.fit_zoom) * factor;
        self.zoom = Some(z.clamp(0.05, 16.0));
    }

    /// Tool buttons for the main toolbar.
    pub fn tools(&mut self, ui: &mut Ui) {
        let tools = [
            (Tool::Pen, Icon::Pen, "Ballpoint pen", Some(self.pen.color)),
            (Tool::Highlighter, Icon::Highlighter, "Highlighter", Some(self.highlighter.color)),
            (Tool::Shapes, Icon::Shapes, "Shapes", Some(self.shape.color)),
            (Tool::Text, Icon::Text, "Text", Some(self.text_style.color)),
            (Tool::Eraser, Icon::Eraser, "Eraser", None),
            (Tool::Crop, Icon::Crop, "Image crop", None),
        ];
        for (tool, icon, tip, accent) in tools {
            if icons::button_ex(ui, icon, self.tool == tool, true, accent, tip).clicked() {
                self.select_tool(tool);
            }
        }
        ui.separator();
        if icons::button_ex(ui, Icon::Undo, false, !self.undo.is_empty(), None, "Undo (Ctrl+Z)").clicked() {
            self.undo();
        }
        if icons::button_ex(ui, Icon::Redo, false, !self.redo.is_empty(), None, "Redo (Ctrl+Y)").clicked() {
            self.redo();
        }
    }

    /// Right-aligned action buttons for the main toolbar (laid out right to left).
    pub fn actions(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        if icons::button(ui, Icon::Save, false, "Save as (Ctrl+S)").clicked() {
            action = Some(Action::Save);
        }
        if icons::button(ui, Icon::Copy, false, "Copy (Ctrl+C)").clicked() {
            action = Some(Action::Copy);
        }
        action
    }

    /// The second toolbar row with options for the current tool.
    pub fn options_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(36.0);
            match self.tool {
                Tool::Pen | Tool::Highlighter => {
                    let (style, palette, range) = if self.tool == Tool::Pen {
                        (&mut self.pen, &PEN_COLORS[..], 1.0..=24.0)
                    } else {
                        (&mut self.highlighter, &HIGHLIGHTER_COLORS[..], 6.0..=48.0)
                    };
                    for &color in palette {
                        if swatch(ui, color, style.color == color).clicked() {
                            style.color = color;
                        }
                    }
                    ui.separator();
                    ui.label("Size");
                    ui.add(egui::Slider::new(&mut style.width, range).integer().suffix(" px"));
                }
                Tool::Shapes => {
                    for (kind, icon, tip) in ShapeKind::ALL {
                        if icons::button(ui, icon, self.shape_kind == kind, tip).clicked() {
                            self.shape_kind = kind;
                        }
                    }
                    ui.separator();
                    for &color in &PEN_COLORS {
                        if swatch(ui, color, self.shape.color == color).clicked() {
                            self.shape.color = color;
                        }
                    }
                    ui.separator();
                    ui.label("Size");
                    ui.add(egui::Slider::new(&mut self.shape.width, 1.0..=24.0).integer().suffix(" px"));
                    ui.label(RichText::new("Shift: square, circle, 15° steps").weak())
                        .on_hover_text("Hold Shift while dragging to draw squares and circles, and snap lines to 15° steps");
                }
                Tool::Text => {
                    for &color in &PEN_COLORS {
                        if swatch(ui, color, self.text_style.color == color).clicked() {
                            self.text_style.color = color;
                        }
                    }
                    ui.separator();
                    ui.label("Size");
                    ui.add(egui::Slider::new(&mut self.text_style.width, 10.0..=120.0).integer().suffix(" px"));
                    ui.label(RichText::new("Click to type, click text to edit, drag to move").weak());
                }
                Tool::Eraser => {
                    ui.label("Click or drag over ink to erase it.");
                    let has_ink = !self.doc.strokes.is_empty() || !self.doc.texts.is_empty();
                    if ui.add_enabled(has_ink, egui::Button::new("Erase all ink")).clicked() {
                        self.push_undo();
                        self.doc.strokes.clear();
                        self.doc.texts.clear();
                    }
                }
                Tool::Crop => {
                    let d = self.crop_draft;
                    ui.label(format!("Drag the edges to crop · {} × {}", d.width().round(), d.height().round()));
                    ui.separator();
                    if icons::button(ui, Icon::Check, false, "Apply crop (Enter)").clicked() {
                        self.apply_crop();
                    }
                    if icons::button(ui, Icon::Close, false, "Cancel (Esc)").clicked() {
                        self.cancel_crop();
                    }
                    if ui.button("Reset").clicked() {
                        self.crop_draft = self.full_rect();
                    }
                }
            }
        });
    }

    pub fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let c = self.doc.crop;
            ui.label(format!("{} × {} px", c.width(), c.height()));
            ui.separator();
            if icons::button(ui, Icon::ZoomOut, false, "Zoom out (Ctrl+-)").clicked() {
                self.zoom_by(0.8);
            }
            let zoom = self.zoom.unwrap_or(self.fit_zoom) * ui.ctx().pixels_per_point();
            ui.label(format!("{:.0}%", zoom * 100.0));
            if icons::button(ui, Icon::ZoomIn, false, "Zoom in (Ctrl++)").clicked() {
                self.zoom_by(1.25);
            }
            if ui.small_button("Fit").on_hover_text("Fit to window (Ctrl+0)").clicked() {
                self.zoom = None;
            }
            if ui.small_button("100%").clicked() {
                self.zoom = Some(1.0 / ui.ctx().pixels_per_point());
            }
            if let Some((text, at)) = &self.status {
                if at.elapsed() < Duration::from_secs(4) {
                    ui.separator();
                    ui.label(text);
                    ui.ctx().request_repaint_after(Duration::from_millis(500));
                } else {
                    self.status = None;
                }
            }
        });
    }

    /// Keyboard shortcuts. Call once per frame before drawing.
    pub fn shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        // Keys belong to the text box while typing.
        if self.editing_text.is_some() {
            return None;
        }
        let shortcut = |m, k| KeyboardShortcut::new(m, k);
        let mut action = None;
        ctx.input_mut(|i| {
            let copy_event = i.events.iter().any(|e| matches!(e, Event::Copy));
            if copy_event || i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::C)) {
                action = Some(Action::Copy);
            }
            if i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::S)) {
                action = Some(Action::Save);
            }
            if i.consume_shortcut(&shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))
                || i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Y))
            {
                self.redo();
            }
            if i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Z)) {
                self.undo();
            }
            if i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Plus))
                || i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Equals))
            {
                self.zoom_by(1.25);
            }
            if i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Minus)) {
                self.zoom_by(0.8);
            }
            if i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Num0)) {
                self.zoom = None;
            }
            if self.tool == Tool::Crop {
                if i.consume_key(Modifiers::NONE, Key::Enter) {
                    self.apply_crop();
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    self.cancel_crop();
                }
            }
        });
        action
    }

    // ---- canvas --------------------------------------------------------

    pub fn canvas(&mut self, ui: &mut Ui) {
        let cropping = self.tool == Tool::Crop;
        let view = if cropping { self.full_rect() } else { self.doc.crop };
        let avail = ui.available_size() - vec2(32.0, 32.0);
        let max_zoom = 1.0 / ui.ctx().pixels_per_point();
        self.fit_zoom = (avail.x / view.width()).min(avail.y / view.height()).clamp(0.02, max_zoom);

        let hovered = ui.rect_contains_pointer(ui.max_rect());
        let pinch = ui.input(|i| i.zoom_delta());
        if hovered && pinch != 1.0 {
            self.zoom_by(pinch);
        }
        let z = self.zoom.unwrap_or(self.fit_zoom);

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            let size = view.size() * z;
            let (outer, response) =
                ui.allocate_exact_size(size.max(ui.available_size()), Sense::click_and_drag());
            let image_rect = Rect::from_center_size(outer.center(), size);
            let to_screen = |p: Pos2| image_rect.min + (p - view.min) * z;
            let to_px = |p: Pos2| view.min + (p - image_rect.min) / z;

            let painter = ui.painter_at(outer);
            painter.rect_filled(image_rect.translate(vec2(2.0, 3.0)), 0.0, Color32::from_black_alpha(60));
            let full = self.full_rect().size();
            let uv = Rect::from_min_max(
                pos2(view.min.x / full.x, view.min.y / full.y),
                pos2(view.max.x / full.x, view.max.y / full.y),
            );
            painter.image(self.texture.id(), image_rect, uv, Color32::WHITE);

            let ink_painter = painter.with_clip_rect(image_rect);
            for stroke in self.doc.strokes.iter().chain(&self.drawing) {
                paint_stroke(&ink_painter, stroke, to_screen, z);
            }
            let editing_index = self.editing_text.as_ref().and_then(|e| e.index);
            for (i, mark) in self.doc.texts.iter().enumerate() {
                if Some(i) != editing_index {
                    mark.paint(&ink_painter, to_screen, z);
                }
            }

            let pointer = response.interact_pointer_pos().or(response.hover_pos());
            let press_origin = ui.input(|i| i.pointer.press_origin());
            match self.tool {
                Tool::Pen | Tool::Highlighter => {
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                    }
                    self.handle_drawing(&response, press_origin.map(to_px), pointer.map(to_px));
                }
                Tool::Shapes => {
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                    }
                    let shift = ui.input(|i| i.modifiers.shift);
                    self.handle_shape(&response, press_origin.map(to_px), pointer.map(to_px), shift);
                }
                Tool::Eraser => {
                    let radius = 8.0 / z;
                    if let Some(p) = pointer {
                        if response.hovered() || response.dragged() {
                            painter.circle_stroke(p, 8.0, Stroke::new(1.5, ui.visuals().strong_text_color()));
                        }
                        if response.dragged() || response.clicked() {
                            self.erase_at(to_px(p), radius, &painter);
                        }
                    }
                    if response.drag_stopped() || response.clicked() {
                        self.erased_this_drag = false;
                    }
                }
                Tool::Text => {
                    self.handle_text(ui, &response, &painter, to_screen, to_px, z);
                }
                Tool::Crop => {
                    self.handle_crop(ui, &response, &painter, image_rect, to_screen, to_px, z);
                }
            }
        });
    }

    fn handle_drawing(&mut self, response: &egui::Response, origin: Option<Pos2>, pointer: Option<Pos2>) {
        let (ink, style) = match self.tool {
            Tool::Highlighter => (Ink::Highlighter, &self.highlighter),
            _ => (Ink::Pen, &self.pen),
        };
        let (color, width) = (style.color, style.width);
        let new_stroke = |start: Pos2| InkStroke { ink, color, width, points: vec![start] };

        if response.clicked()
            && let Some(p) = pointer
        {
            self.push_undo();
            self.doc.strokes.push(new_stroke(p));
            return;
        }
        if response.drag_started()
            && let Some(start) = origin
        {
            self.drawing = Some(new_stroke(start));
        }
        if response.dragged()
            && let (Some(stroke), Some(p)) = (&mut self.drawing, pointer)
            && stroke.points.last().is_none_or(|last| last.distance(p) >= 0.75)
        {
            stroke.points.push(p);
        }
        if response.drag_stopped()
            && let Some(stroke) = self.drawing.take()
        {
            self.push_undo();
            self.doc.strokes.push(stroke);
        }
    }

    fn handle_shape(&mut self, response: &egui::Response, origin: Option<Pos2>, pointer: Option<Pos2>, shift: bool) {
        if response.drag_started()
            && let Some(start) = origin
        {
            self.shape_start = Some(start);
        }
        if response.dragged()
            && let (Some(start), Some(p)) = (self.shape_start, pointer)
        {
            let points = self.shape_kind.points(start, p, self.shape.width, shift);
            self.drawing =
                Some(InkStroke { ink: Ink::Shape, color: self.shape.color, width: self.shape.width, points });
        }
        if response.drag_stopped() {
            self.shape_start = None;
            if let Some(stroke) = self.drawing.take() {
                self.push_undo();
                self.doc.strokes.push(stroke);
            }
        }
    }

    fn erase_at(&mut self, p: Pos2, radius: f32, painter: &Painter) {
        let text_hit = |t: &TextMark| t.bounds(painter).expand(radius).contains(p);
        if !self.doc.strokes.iter().any(|s| s.hit(p, radius)) && !self.doc.texts.iter().any(text_hit) {
            return;
        }
        // One undo step per eraser drag.
        if !self.erased_this_drag {
            self.push_undo();
            self.erased_this_drag = true;
        }
        self.doc.strokes.retain(|s| !s.hit(p, radius));
        self.doc.texts.retain(|t| !text_hit(t));
    }

    /// Finishes the text label being typed, if any. Empty labels are dropped.
    pub fn commit_text(&mut self) {
        let Some(EditingText { index, mut mark, .. }) = self.editing_text.take() else { return };
        mark.text = mark.text.trim_end().to_owned();
        let empty = mark.text.trim().is_empty();
        match index {
            Some(i) if empty => {
                self.push_undo();
                self.doc.texts.remove(i);
            }
            Some(i) if self.doc.texts[i] != mark => {
                self.push_undo();
                self.doc.texts[i] = mark;
            }
            None if !empty => {
                self.push_undo();
                self.doc.texts.push(mark);
            }
            _ => {}
        }
    }

    fn handle_text(
        &mut self,
        ui: &mut Ui,
        response: &egui::Response,
        painter: &Painter,
        to_screen: impl Fn(Pos2) -> Pos2,
        to_px: impl Fn(Pos2) -> Pos2,
        z: f32,
    ) {
        let hit = |texts: &[TextMark], p: Pos2| texts.iter().rposition(|t| t.bounds(painter).expand(4.0 / z).contains(p));

        if let Some(p) = response.hover_pos().map(&to_px) {
            let over_text = self.editing_text.is_none() && hit(&self.doc.texts, p).is_some();
            ui.ctx().set_cursor_icon(if over_text { CursorIcon::Move } else { CursorIcon::Text });
        }

        // Drag a label to move it.
        if response.drag_started()
            && self.editing_text.is_none()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin()).map(&to_px)
            && let Some(i) = hit(&self.doc.texts, origin)
        {
            self.push_undo();
            self.moving_text = Some(i);
        }
        if response.dragged()
            && let Some(i) = self.moving_text
        {
            self.doc.texts[i].pos += response.drag_delta() / z;
        }
        if response.drag_stopped() {
            self.moving_text = None;
        }

        // Click to start a new label or edit an existing one.
        if response.clicked() {
            if self.editing_text.is_some() {
                self.commit_text();
            } else if let Some(p) = response.interact_pointer_pos().map(&to_px) {
                let (index, mark) = match hit(&self.doc.texts, p) {
                    Some(i) => (Some(i), self.doc.texts[i].clone()),
                    None => {
                        let style = &self.text_style;
                        let pos = p - vec2(0.0, style.width * 0.6);
                        (None, TextMark { pos, text: String::new(), color: style.color, size: style.width })
                    }
                };
                self.text_style = InkStyle { color: mark.color, width: mark.size };
                self.editing_text = Some(EditingText { index, mark, request_focus: true });
            }
        }

        let Some(edit) = &mut self.editing_text else { return };
        // The options bar edits the label being typed.
        edit.mark.color = self.text_style.color;
        edit.mark.size = self.text_style.width;

        let mark = &mut edit.mark;
        let bounds = mark.bounds(painter);
        let rect = Rect::from_min_max(to_screen(bounds.min), to_screen(bounds.max));
        // Leave room for the next character so the box grows as you type.
        let rect = Rect::from_min_size(rect.min, rect.size() + vec2(mark.size * z, 0.0));
        painter.rect_stroke(rect.expand(4.0), 3.0, Stroke::new(1.0, ACCENT), StrokeKind::Outside);
        mark.paint(painter, &to_screen, z);

        // An invisible text box handles typing, the caret and selection; the
        // label itself is painted above, exactly as it will be saved.
        let text_edit = egui::TextEdit::multiline(&mut mark.text)
            .font(text::font_id(mark.size * z))
            .text_color(Color32::TRANSPARENT)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(rect.width())
            .desired_rows(1);
        let field = ui.put(rect, text_edit);
        if edit.request_focus {
            field.request_focus();
            edit.request_focus = false;
        } else if field.lost_focus() {
            self.commit_text();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_crop(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        painter: &Painter,
        image_rect: Rect,
        to_screen: impl Fn(Pos2) -> Pos2,
        to_px: impl Fn(Pos2) -> Pos2,
        z: f32,
    ) {
        let draft = Rect::from_min_max(to_screen(self.crop_draft.min), to_screen(self.crop_draft.max));
        let grab_at = |p: Pos2| {
            const NEAR: f32 = 10.0;
            let in_y = p.y > draft.top() - NEAR && p.y < draft.bottom() + NEAR;
            let in_x = p.x > draft.left() - NEAR && p.x < draft.right() + NEAR;
            let left = in_y && (p.x - draft.left()).abs() < NEAR;
            let right = in_y && !left && (p.x - draft.right()).abs() < NEAR;
            let top = in_x && (p.y - draft.top()).abs() < NEAR;
            let bottom = in_x && !top && (p.y - draft.bottom()).abs() < NEAR;
            if left || right || top || bottom {
                Grab::Edges { left, right, top, bottom }
            } else if draft.contains(p) {
                Grab::Move
            } else {
                Grab::New(to_px(p))
            }
        };

        // Cursor feedback.
        if let Some(p) = response.hover_pos() {
            let icon = match self.crop_grab.unwrap_or_else(|| grab_at(p)) {
                Grab::Edges { left, right, top, bottom } => match (left || right, top || bottom) {
                    (true, true) if (left && top) || (right && bottom) => CursorIcon::ResizeNwSe,
                    (true, true) => CursorIcon::ResizeNeSw,
                    (true, false) => CursorIcon::ResizeHorizontal,
                    _ => CursorIcon::ResizeVertical,
                },
                Grab::Move => CursorIcon::Grab,
                Grab::New(_) => CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        if response.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin());
            self.crop_grab = origin.map(grab_at);
        }
        if response.dragged()
            && let (Some(grab), Some(p)) = (self.crop_grab, response.interact_pointer_pos())
        {
            let full = self.full_rect();
            let p = to_px(p).clamp(full.min, full.max);
            let d = &mut self.crop_draft;
            match grab {
                Grab::New(start) => *d = Rect::from_two_pos(start, p),
                Grab::Move => {
                    let delta = response.drag_delta() / z;
                    let moved = d.translate(delta);
                    let shift = vec2(
                        (full.min.x - moved.min.x).max(0.0) + (full.max.x - moved.max.x).min(0.0),
                        (full.min.y - moved.min.y).max(0.0) + (full.max.y - moved.max.y).min(0.0),
                    );
                    *d = moved.translate(shift);
                }
                Grab::Edges { left, right, top, bottom } => {
                    if left {
                        d.min.x = p.x.min(d.max.x - 1.0);
                    }
                    if right {
                        d.max.x = p.x.max(d.min.x + 1.0);
                    }
                    if top {
                        d.min.y = p.y.min(d.max.y - 1.0);
                    }
                    if bottom {
                        d.max.y = p.y.max(d.min.y + 1.0);
                    }
                }
            }
        }
        if response.drag_stopped() {
            self.crop_grab = None;
        }

        // Dim outside the crop, then draw the frame, thirds and corner handles.
        let draft = Rect::from_min_max(to_screen(self.crop_draft.min), to_screen(self.crop_draft.max));
        let dim = Color32::from_black_alpha(150);
        for part in [
            Rect::from_min_max(image_rect.min, pos2(image_rect.max.x, draft.min.y)),
            Rect::from_min_max(pos2(image_rect.min.x, draft.max.y), image_rect.max),
            Rect::from_min_max(pos2(image_rect.min.x, draft.min.y), pos2(draft.min.x, draft.max.y)),
            Rect::from_min_max(pos2(draft.max.x, draft.min.y), pos2(image_rect.max.x, draft.max.y)),
        ] {
            if part.is_positive() {
                painter.rect_filled(part, 0.0, dim);
            }
        }
        let thin = Stroke::new(1.0, Color32::from_white_alpha(90));
        for t in [1.0 / 3.0, 2.0 / 3.0] {
            let x = draft.left() + draft.width() * t;
            let y = draft.top() + draft.height() * t;
            painter.line_segment([pos2(x, draft.top()), pos2(x, draft.bottom())], thin);
            painter.line_segment([pos2(draft.left(), y), pos2(draft.right(), y)], thin);
        }
        painter.rect_stroke(draft, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Middle);
        let handle = Stroke::new(4.0, Color32::WHITE);
        let len = 16.0_f32.min(draft.width() / 2.0).min(draft.height() / 2.0);
        for (corner, dx, dy) in [
            (draft.left_top(), 1.0, 1.0),
            (draft.right_top(), -1.0, 1.0),
            (draft.left_bottom(), 1.0, -1.0),
            (draft.right_bottom(), -1.0, -1.0),
        ] {
            painter.add(Shape::line(vec![corner + vec2(0.0, len * dy), corner, corner + vec2(len * dx, 0.0)], handle));
        }
    }
}

fn paint_stroke(painter: &Painter, stroke: &InkStroke, to_screen: impl Fn(Pos2) -> Pos2, z: f32) {
    let color = stroke.display_color();
    let width = (stroke.width * z).max(1.0);
    let points: Vec<Pos2> = stroke.points.iter().map(|&p| to_screen(p)).collect();
    if points.len() == 1 {
        painter.circle_filled(points[0], width / 2.0, color);
        return;
    }
    if stroke.ink == Ink::Shape {
        // Separate segments with round joints, so arrow heads that double
        // back on themselves don't produce miter spikes.
        let stroke_style = Stroke::new(width, color);
        for seg in points.windows(2) {
            painter.line_segment([seg[0], seg[1]], stroke_style);
        }
        for &p in &points {
            painter.circle_filled(p, width / 2.0, color);
        }
        return;
    }
    if stroke.ink == Ink::Pen {
        // Round caps.
        painter.circle_filled(points[0], width / 2.0, color);
        painter.circle_filled(points[points.len() - 1], width / 2.0, color);
    }
    painter.add(Shape::line(points, Stroke::new(width, color)));
}

fn swatch(ui: &mut Ui, color: Color32, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let painter = ui.painter();
    let c = rect.center();
    if selected {
        painter.circle_stroke(c, 13.0, Stroke::new(2.0, ACCENT));
    } else if response.hovered() {
        painter.circle_stroke(c, 13.0, Stroke::new(1.0, ui.visuals().weak_text_color()));
    }
    painter.circle_filled(c, 10.0, color);
    painter.circle_stroke(c, 10.0, Stroke::new(1.0, Color32::from_gray(128)));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_makes_squares_and_snaps_lines() {
        let square = ShapeKind::Rectangle.points(pos2(10.0, 10.0), pos2(40.0, 20.0), 4.0, true);
        assert_eq!(square[2], pos2(40.0, 40.0));
        let line = ShapeKind::Line.points(pos2(0.0, 0.0), pos2(100.0, 3.0), 4.0, true);
        assert!((line[1].y).abs() < 1e-3, "{:?}", line[1]);
    }

    #[test]
    fn arrow_has_head_through_tip_and_hits() {
        let arrow = ShapeKind::Arrow.points(pos2(0.0, 0.0), pos2(100.0, 0.0), 4.0, false);
        assert_eq!(arrow.len(), 5);
        assert_eq!(arrow[1], pos2(100.0, 0.0));
        assert!(arrow[2].x < 100.0 && arrow[4].x < 100.0);
        let stroke = InkStroke { ink: Ink::Shape, color: Color32::RED, width: 4.0, points: arrow };
        assert!(stroke.hit(pos2(50.0, 1.0), 2.0));
        assert!(!stroke.hit(pos2(50.0, 30.0), 2.0));
    }
}
