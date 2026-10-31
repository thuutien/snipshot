//! Look and feel: larger text and controls than egui's defaults, so the app
//! is easy to read and click, plus the light/dark theme choice.

use eframe::egui::{self, FontFamily, FontId, Margin, TextStyle, ThemePreference, vec2};

use crate::settings::Theme;

/// Applies the sizes to both the light and dark styles, then picks the theme.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(15.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(15.5, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(21.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(14.5, FontFamily::Monospace)),
        ]
        .into();
        let spacing = &mut style.spacing;
        spacing.item_spacing = vec2(8.0, 6.0);
        spacing.button_padding = vec2(8.0, 4.0);
        spacing.interact_size = vec2(44.0, 26.0);
        spacing.menu_margin = Margin::same(10);
        spacing.slider_width = 130.0;
        spacing.icon_width = 18.0;
        spacing.icon_width_inner = 10.0;
        spacing.icon_spacing = 6.0;
    });
    set_theme(ctx, theme);
}

pub fn set_theme(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(match theme {
        Theme::System => ThemePreference::System,
        Theme::Light => ThemePreference::Light,
        Theme::Dark => ThemePreference::Dark,
    });
}
