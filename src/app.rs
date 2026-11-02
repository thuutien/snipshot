//! The Snipshot window: home screen, snip overlay and editor, which all share
//! one native window that switches between normal and full-screen.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Button, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText, Ui, Vec2, ViewportCommand,
    WindowLevel, pos2, vec2,
};
use image::RgbaImage;

use crate::capture::{self, Area, Capture};
use crate::editor::{Action, Editor};
use crate::icons::Icon;
use crate::overlay::{ACCENT, Outcome, Overlay};
use crate::raise;
use crate::settings::{Settings, Theme};
use crate::style;
use crate::updater::{self, Install, Progress, UpdateStatus};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnipMode {
    Rectangle,
    Window,
    Fullscreen,
    Freeform,
}

impl SnipMode {
    pub const ALL: [SnipMode; 4] = [Self::Rectangle, Self::Window, Self::Fullscreen, Self::Freeform];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rectangle => "Rectangle",
            Self::Window => "Window",
            Self::Fullscreen => "Full screen",
            Self::Freeform => "Freeform",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::Rectangle => Icon::Rectangle,
            Self::Window => Icon::Window,
            Self::Fullscreen => Icon::Fullscreen,
            Self::Freeform => Icon::Freeform,
        }
    }

    pub fn area(self) -> Area {
        if self == Self::Fullscreen { Area::AllMonitors } else { Area::CursorMonitor }
    }
}

const DELAYS: [u32; 4] = [0, 3, 5, 10];
const HOME_SIZE: Vec2 = vec2(700.0, 240.0);

fn delay_label(seconds: u32) -> String {
    if seconds == 0 { "No delay".to_owned() } else { format!("{seconds} s delay") }
}

enum Screen {
    Home,
    /// Window hidden while we wait for the delay and the capture.
    Waiting,
    /// Capture done; the overlay or editor is built on the next frame.
    Captured(Capture),
    Overlay(Overlay),
    Editor(Editor),
}

pub struct SnipApp {
    screen: Screen,
    /// The editor to return to if a new snip is cancelled.
    previous_editor: Option<Editor>,
    mode: SnipMode,
    delay: u32,
    settings: Settings,
    clipboard: Option<arboard::Clipboard>,
    pending: Option<Receiver<Result<Capture, String>>>,
    /// Started from the command line: cancelling the snip quits.
    exit_on_cancel: bool,
    error: Option<String>,
    update_rx: Option<Receiver<UpdateStatus>>,
    update: Option<UpdateStatus>,
    /// The user started the check, so show "up to date" and errors too.
    update_check_is_manual: bool,
    update_banner_closed: bool,
    /// How this copy is installed, once an update has been started.
    install: Option<Install>,
    install_rx: Option<Receiver<Progress>>,
    install_progress: Option<Progress>,
    /// The automatic check runs on the first frame, once a context exists.
    startup_check_pending: bool,
    /// Our X11 window, for bringing the editor to the front.
    window_id: Option<u32>,
    /// Remaining attempts to raise the window, and when to make the next one.
    raise: Option<(u8, Instant)>,
    /// The size we asked for, remaining attempts, and when to check next.
    /// Coming out of full screen, the compositor may put back the old size.
    resize: Option<(Vec2, u8, Instant)>,
}

impl SnipApp {
    pub fn new(start: Option<(SnipMode, Capture)>, delay: u32, window_id: Option<u32>) -> Self {
        let mut app = Self {
            screen: Screen::Home,
            previous_editor: None,
            mode: SnipMode::Rectangle,
            delay,
            settings: Settings::load(),
            clipboard: None,
            pending: None,
            exit_on_cancel: false,
            error: None,
            update_rx: None,
            update: None,
            update_check_is_manual: false,
            update_banner_closed: false,
            install: None,
            install_rx: None,
            install_progress: None,
            startup_check_pending: true,
            window_id,
            raise: None,
            resize: None,
        };
        if let Some((mode, capture)) = start {
            app.mode = mode;
            app.screen = Screen::Captured(capture);
            app.exit_on_cancel = true;
        }
        app
    }

    pub fn theme(&self) -> Theme {
        self.settings.theme
    }

    fn start_snip(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        if let Screen::Editor(_) = self.screen
            && let Screen::Editor(editor) = std::mem::replace(&mut self.screen, Screen::Waiting)
        {
            self.previous_editor = Some(editor);
        }
        self.screen = Screen::Waiting;
        self.error = None;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        // Wayland ignores hiding a window, so also drop the title bar and draw
        // nothing at all (see `clear_color`) to keep the window out of the snip.
        ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
        ctx.request_repaint();

        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let (delay, area) = (self.delay, self.mode.area());
        std::thread::spawn(move || {
            // Give the compositor time to actually hide our window.
            std::thread::sleep(Duration::from_millis(350) + Duration::from_secs(delay as u64));
            let _ = tx.send(capture::capture(area));
            ctx.request_repaint();
        });
        self.pending = Some(rx);
    }

    /// Polls the background capture. Runs even while the window is hidden.
    fn poll_capture(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.pending else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("screen capture stopped unexpectedly".to_owned()),
        };
        self.pending = None;
        match result {
            Ok(capture) => {
                if self.mode == SnipMode::Fullscreen {
                    show_normal_window(ctx, HOME_SIZE);
                } else {
                    show_overlay_window(ctx, capture.origin);
                }
                self.screen = Screen::Captured(capture);
            }
            Err(e) => {
                self.error = Some(e);
                self.cancel(ctx);
            }
        }
    }

    fn check_for_updates(&mut self, ctx: &egui::Context, manual: bool) {
        if self.update_rx.is_some() {
            return;
        }
        self.update_check_is_manual = manual;
        self.update = None;
        self.update_rx = Some(updater::spawn_check(ctx));
    }

    /// Checks automatically at most once a day, if enabled.
    fn maybe_check_on_startup(&mut self, ctx: &egui::Context) {
        if !std::mem::take(&mut self.startup_check_pending) || !self.settings.check_for_updates {
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if now.saturating_sub(self.settings.last_update_check) < 24 * 60 * 60 {
            return;
        }
        self.settings.last_update_check = now;
        let _ = self.settings.save();
        self.check_for_updates(ctx, false);
    }

    fn poll_update(&mut self) {
        let Some(rx) = &self.update_rx else { return };
        let status = match rx.try_recv() {
            Ok(status) => status,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => UpdateStatus::Failed("The update check stopped unexpectedly".to_owned()),
        };
        self.update_rx = None;
        // A silent background check only speaks up when there is an update.
        if self.update_check_is_manual || matches!(status, UpdateStatus::Available { .. }) {
            self.update_banner_closed = false;
            self.update = Some(status);
        }
    }

    /// Downloads and installs the available update in the background.
    fn start_update(&mut self, ctx: &egui::Context) {
        let Some(UpdateStatus::Available { assets, .. }) = &self.update else { return };
        if self.install_rx.is_some() {
            return;
        }
        self.update_banner_closed = false;
        match Install::detect() {
            Ok(install) => {
                self.install_rx = Some(updater::spawn_install(ctx, install.clone(), assets.clone()));
                self.install = Some(install);
                self.install_progress = Some(Progress::Downloading(0.0));
            }
            Err(e) => self.install_progress = Some(Progress::Failed(e)),
        }
    }

    fn poll_install(&mut self) {
        let Some(rx) = &self.install_rx else { return };
        loop {
            match rx.try_recv() {
                Ok(progress) => self.install_progress = Some(progress),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        self.install_rx = None;
        if !matches!(self.install_progress, Some(Progress::Done | Progress::Failed(_))) {
            self.install_progress = Some(Progress::Failed("The update stopped unexpectedly".to_owned()));
        }
    }

    /// Starts the updated copy and closes this one.
    fn relaunch(&mut self, ctx: &egui::Context) {
        let Some(install) = &self.install else { return };
        match install.relaunch() {
            Ok(()) => ctx.send_viewport_cmd(ViewportCommand::Close),
            Err(e) => self.install_progress = Some(Progress::Failed(e)),
        }
    }

    /// A strip under the toolbar announcing an update and its install
    /// progress, or the result of a manual check (the menu closes when its
    /// button is clicked).
    fn update_banner(&mut self, ui: &mut Ui) {
        let Some(status) = self.update.clone() else { return };
        let busy = self.install_rx.is_some();
        if self.update_banner_closed && !busy {
            return;
        }
        let (green, orange) = (Color32::from_rgb(16, 124, 16), Color32::from_rgb(200, 120, 0));
        let tint = match (&status, &self.install_progress) {
            (UpdateStatus::Failed(_), _) | (_, Some(Progress::Failed(_))) => orange,
            (UpdateStatus::UpToDate, _) | (_, Some(Progress::Done)) => green,
            _ => ACCENT,
        };
        let (mut start, mut relaunch) = (false, false);
        // A light wash of the tint over the panel color, so text stays
        // readable in both the light and dark themes.
        let fill = ui.visuals().panel_fill.lerp_to_gamma(tint, 0.25);
        egui::Panel::top("update-banner")
            .frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    match &status {
                        UpdateStatus::UpToDate => {
                            ui.label(
                                RichText::new(format!("You're on the latest version ({})", updater::current_version()))
                                    .strong(),
                            );
                        }
                        UpdateStatus::Failed(e) => {
                            ui.label(RichText::new(format!("Couldn't check for updates: {e}")).strong());
                        }
                        UpdateStatus::Available { version, url, .. } => match &self.install_progress {
                            None => {
                                ui.label(RichText::new(format!("Snipshot {version} is available")).strong());
                                ui.label(RichText::new(format!("(you have {})", updater::current_version())).weak());
                                start = ui.button("Update now").clicked();
                                if ui.link("What's new").clicked() {
                                    updater::open_url(url);
                                }
                            }
                            Some(Progress::Downloading(fraction)) => {
                                ui.label(RichText::new(format!("Downloading Snipshot {version}…")).strong());
                                ui.add(egui::ProgressBar::new(*fraction).desired_width(220.0).show_percentage());
                            }
                            Some(Progress::Installing) => {
                                ui.spinner();
                                ui.label(RichText::new(format!("Installing Snipshot {version}…")).strong());
                                if self.install.as_ref().is_some_and(Install::asks_for_password) {
                                    ui.label(RichText::new("Enter your password if asked.").weak());
                                }
                            }
                            Some(Progress::Done) => {
                                ui.label(RichText::new(format!("Snipshot {version} is installed.")).strong());
                                relaunch = ui
                                    .button("Restart now")
                                    .on_hover_text("Unsaved markup in the current snip will be lost")
                                    .clicked();
                            }
                            Some(Progress::Failed(e)) => {
                                ui.label(RichText::new(format!("Couldn't update: {e}")).strong());
                                start = ui.button("Try again").clicked();
                                if ui.link("Download page").clicked() {
                                    updater::open_url(url);
                                }
                            }
                        },
                    }
                    if !busy {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                                self.update_banner_closed = true;
                            }
                        });
                    }
                });
            });
        if start {
            self.install_progress = None;
            self.start_update(ui.ctx());
        }
        if relaunch {
            self.relaunch(ui.ctx());
        }
    }

    /// Shows an error where the user will see it: the editor's status bar,
    /// or the home screen.
    fn show_error(&mut self, error: String) {
        match &mut self.screen {
            Screen::Editor(editor) => editor.set_error(error),
            _ => self.error = Some(error),
        }
    }

    fn cancel(&mut self, ctx: &egui::Context) {
        if self.exit_on_cancel {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        self.screen = match self.previous_editor.take() {
            Some(mut editor) => {
                // The home screen isn't showing, so the editor reports the error.
                if let Some(error) = self.error.take() {
                    editor.set_error(error);
                }
                self.show_normal(ctx, editor.preferred_window_size(ctx));
                Screen::Editor(editor)
            }
            None => {
                self.show_normal(ctx, HOME_SIZE);
                Screen::Home
            }
        };
    }

    fn finish(&mut self, ctx: &egui::Context, image: RgbaImage) {
        self.exit_on_cancel = false;
        self.previous_editor = None;
        let mut notes = Vec::new();
        if self.settings.copy_to_clipboard {
            match self.copy_image(&image) {
                Ok(()) => notes.push("Copied to clipboard".to_owned()),
                Err(e) => notes.push(format!("Couldn't copy: {e}")),
            }
        }
        if self.settings.auto_save {
            let path = self.settings.save_dir.join(default_file_name());
            match std::fs::create_dir_all(&self.settings.save_dir).map_err(|e| e.to_string()).and_then(|_| save_image(&image, &path)) {
                Ok(path) => notes.push(format!("Saved to {}", path.display())),
                Err(e) => notes.push(format!("Couldn't save: {e}")),
            }
        }
        let mut editor = Editor::new(ctx, image);
        editor.set_status(notes.join(" · "));
        self.show_normal(ctx, editor.preferred_window_size(ctx));
        self.screen = Screen::Editor(editor);
        // The window manager applies the size and mapping over a few frames,
        // so ask to be raised a few times while that settles.
        self.raise = Some((3, Instant::now() + Duration::from_millis(120)));
    }

    /// Leaves full screen at `size`, and keeps asking for that size until the
    /// window has it.
    fn show_normal(&mut self, ctx: &egui::Context, size: Vec2) {
        show_normal_window(ctx, size);
        self.resize = Some((size, 8, Instant::now() + Duration::from_millis(150)));
    }

    fn keep_size(&mut self, ctx: &egui::Context) {
        let Some((size, attempts, at)) = self.resize else { return };
        let now = Instant::now();
        if now < at {
            ctx.request_repaint_after(at - now);
            return;
        }
        let current = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size());
        if attempts == 0 || current.is_some_and(|c| (c - size).length() < 2.0) {
            self.resize = None;
            return;
        }
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        self.resize = Some((size, attempts - 1, now + Duration::from_millis(150)));
        ctx.request_repaint_after(Duration::from_millis(150));
    }

    fn raise_window(&mut self, ctx: &egui::Context) {
        let Some((attempts, at)) = self.raise else { return };
        let now = Instant::now();
        if now < at {
            ctx.request_repaint_after(at - now);
            return;
        }
        match self.window_id {
            Some(id) => raise::activate_x11(id),
            None => ctx.send_viewport_cmd(ViewportCommand::Focus),
        }
        self.raise = (attempts > 1).then(|| (attempts - 1, now + Duration::from_millis(250)));
        if self.raise.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn copy_image(&mut self, image: &RgbaImage) -> Result<(), String> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        let clipboard = self.clipboard.as_mut().expect("clipboard was just created");
        clipboard
            .set_image(arboard::ImageData {
                width: image.width() as usize,
                height: image.height() as usize,
                bytes: Cow::Borrowed(image.as_raw()),
            })
            .map_err(|e| e.to_string())
    }

    fn run_editor_action(&mut self, action: Action) {
        let Screen::Editor(editor) = &mut self.screen else { return };
        editor.commit_text();
        let image = editor.render();
        let status = match action {
            Action::Copy => match self.copy_image(&image) {
                Ok(()) => "Copied to clipboard".to_owned(),
                Err(e) => format!("Couldn't copy: {e}"),
            },
            Action::Save => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save As")
                    .set_file_name(default_file_name())
                    .add_filter("PNG", &["png"])
                    .add_filter("JPEG", &["jpg", "jpeg"]);
                if self.settings.save_dir.is_dir() {
                    dialog = dialog.set_directory(&self.settings.save_dir);
                }
                match dialog.save_file() {
                    Some(path) => match save_image(&image, &path) {
                        Ok(path) => format!("Saved to {}", path.display()),
                        Err(e) => format!("Couldn't save: {e}"),
                    },
                    None => return,
                }
            }
        };
        if let Screen::Editor(editor) = &mut self.screen {
            editor.set_status(status);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) -> (bool, Option<Action>) {
        let mut new_snip = false;
        let mut action = None;
        // The settings and action buttons go on the right first, so they always
        // stay reachable; the rest scrolls sideways if the window is too narrow.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.set_min_height(40.0);
            self.settings_menu(ui);
            if let Screen::Editor(editor) = &mut self.screen {
                action = editor.actions(ui);
            }
            egui::ScrollArea::horizontal().id_salt("toolbar-scroll").auto_shrink([false, true]).show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    new_snip = self.toolbar_left(ui);
                });
            });
        });
        (new_snip, action)
    }

    /// New, snip mode, delay and (in the editor) the markup tools.
    fn toolbar_left(&mut self, ui: &mut Ui) -> bool {
        let mut new_snip = false;
        let new = Button::new(RichText::new("+  New").color(Color32::WHITE).strong())
            .fill(ACCENT)
            .min_size(vec2(88.0, 36.0))
            .corner_radius(6);
        if ui.add(new).on_hover_text("New snip (Ctrl+N)").clicked() {
            new_snip = true;
        }
        egui::ComboBox::from_id_salt("snip-mode")
            .selected_text(self.mode.label())
            .width(130.0)
            .show_ui(ui, |ui| {
                for mode in SnipMode::ALL {
                    ui.selectable_value(&mut self.mode, mode, mode.label());
                }
            })
            .response
            .on_hover_text("Snip mode");
        egui::ComboBox::from_id_salt("snip-delay")
            .selected_text(delay_label(self.delay))
            .width(115.0)
            .show_ui(ui, |ui| {
                for delay in DELAYS {
                    ui.selectable_value(&mut self.delay, delay, delay_label(delay));
                }
            })
            .response
            .on_hover_text("Delay before the snip");

        if let Screen::Editor(editor) = &mut self.screen {
            ui.separator();
            editor.tools(ui);
        }
        new_snip
    }

    fn settings_menu(&mut self, ui: &mut Ui) {
        ui.menu_button(RichText::new("•••").strong().size(18.0), |ui| {
            let mut changed = false;
            changed |= ui.checkbox(&mut self.settings.copy_to_clipboard, "Copy snips to the clipboard automatically").changed();
            changed |= ui.checkbox(&mut self.settings.auto_save, "Save snips automatically").changed();
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.settings.save_dir.display().to_string()).weak());
                if ui.button("Change…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_title("Save snips to").pick_folder()
                {
                    self.settings.save_dir = dir;
                    changed = true;
                }
            });
            changed |= ui.checkbox(&mut self.settings.check_for_updates, "Check for updates automatically").changed();
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Theme");
                for theme in Theme::ALL {
                    if ui.selectable_value(&mut self.settings.theme, theme, theme.label()).changed() {
                        style::set_theme(ui.ctx(), theme);
                        changed = true;
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Snipshot {}", updater::current_version())).weak());
                let checking = self.update_rx.is_some();
                if ui.add_enabled(!checking, Button::new("Check for updates")).clicked() {
                    self.check_for_updates(ui.ctx(), true);
                }
                if checking {
                    ui.spinner();
                }
            });
            let mut start_update = false;
            match &self.update {
                Some(UpdateStatus::UpToDate) => {
                    ui.label("You're on the latest version.");
                }
                Some(UpdateStatus::Available { version, .. }) => {
                    ui.horizontal(|ui| {
                        ui.label(format!("Version {version} is available."));
                        if self.install_progress.is_none() {
                            start_update = ui.button("Update now").clicked();
                        }
                    });
                }
                Some(UpdateStatus::Failed(e)) => {
                    ui.colored_label(ui.visuals().warn_fg_color, e);
                }
                None => {}
            }
            if start_update {
                // Progress shows in the banner.
                self.start_update(ui.ctx());
                ui.close();
            }
            if changed && let Err(e) = self.settings.save() {
                self.show_error(format!("Couldn't save settings: {e}"));
            }
        })
        .response
        .on_hover_text("Settings");
    }

    fn home(&self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 2.0 - 40.0);
            ui.label(RichText::new("Select the snip mode, then click New to start a snip.").size(18.0));
            ui.add_space(6.0);
            ui.label(
                RichText::new("Tip: bind a keyboard shortcut to `snipshot --rect` to snip instantly.").weak(),
            );
            if let Some(error) = &self.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    }
}

impl eframe::App for SnipApp {
    /// Fully transparent while waiting for a capture, opaque otherwise.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if matches!(self.screen, Screen::Waiting) { [0.0; 4] } else { visuals.panel_fill.to_normalized_gamma_f32() }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_capture(ctx);
        self.poll_update();
        self.poll_install();
        self.maybe_check_on_startup(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if matches!(self.screen, Screen::Captured(_))
            && let Screen::Captured(capture) = std::mem::replace(&mut self.screen, Screen::Waiting)
        {
            if self.mode == SnipMode::Fullscreen {
                self.finish(&ctx, capture.image);
            } else {
                self.screen = Screen::Overlay(Overlay::new(&ctx, capture, self.mode));
            }
        }

        if let Screen::Overlay(overlay) = &mut self.screen {
            let outcome = overlay.ui(ui);
            self.mode = overlay.mode;
            match outcome {
                Outcome::Pending => {}
                Outcome::Cancel => self.cancel(&ctx),
                Outcome::Done(image) => self.finish(&ctx, image),
            }
            return;
        }
        if matches!(self.screen, Screen::Waiting) {
            return;
        }

        self.keep_size(&ctx);
        self.raise_window(&ctx);
        let mut action = None;
        let mut new_snip = ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::N)));
        if let Screen::Editor(editor) = &mut self.screen {
            action = editor.shortcuts(&ctx);
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            let (clicked_new, clicked_action) = self.toolbar(ui);
            new_snip |= clicked_new;
            action = action.take().or(clicked_action);
        });
        self.update_banner(ui);

        match &mut self.screen {
            Screen::Editor(editor) => {
                egui::Panel::top("tool-options").show(ui, |ui| editor.options_bar(ui));
                egui::Panel::bottom("status").show(ui, |ui| editor.status_bar(ui));
                // A gray backdrop in light mode so white edges of the snip show.
                let backdrop =
                    if ui.visuals().dark_mode { ui.visuals().extreme_bg_color } else { Color32::from_gray(222) };
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(backdrop))
                    .show(ui, |ui| editor.canvas(ui));
            }
            _ => {
                egui::CentralPanel::default().show(ui, |ui| self.home(ui));
            }
        }

        if let Some(action) = action {
            self.run_editor_action(action);
        }
        if new_snip {
            self.start_snip(&ctx);
        }
    }
}

fn show_overlay_window(ctx: &egui::Context, origin: (i32, i32)) {
    let ppp = ctx.native_pixels_per_point().unwrap_or(1.0);
    ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(origin.0 as f32 / ppp, origin.1 as f32 / ppp)));
    ctx.send_viewport_cmd(ViewportCommand::Fullscreen(true));
    ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
}

fn show_normal_window(ctx: &egui::Context, size: Vec2) {
    ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
    ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
    ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
}

fn default_file_name() -> String {
    chrono::Local::now().format("Screenshot %Y-%m-%d %H%M%S.png").to_string()
}

/// Saves as JPEG or PNG depending on the extension (PNG if there is none).
fn save_image(image: &RgbaImage, path: &Path) -> Result<PathBuf, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let path = match ext.as_deref() {
        Some("png" | "jpg" | "jpeg") => path.to_path_buf(),
        _ => path.with_extension("png"),
    };
    let result = if matches!(ext.as_deref(), Some("jpg" | "jpeg")) {
        image::DynamicImage::ImageRgba8(image.clone()).to_rgb8().save(&path)
    } else {
        image.save(&path)
    };
    result.map(|()| path).map_err(|e| e.to_string())
}
