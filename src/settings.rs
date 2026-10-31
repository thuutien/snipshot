//! User settings, stored as `key=value` lines in ~/.config/snipshot/settings.conf.

use std::path::PathBuf;

/// The color theme of the Snipshot window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

pub struct Settings {
    pub copy_to_clipboard: bool,
    pub auto_save: bool,
    pub save_dir: PathBuf,
    pub check_for_updates: bool,
    pub theme: Theme,
    /// Unix time of the last automatic update check.
    pub last_update_check: u64,
}

impl Default for Settings {
    fn default() -> Self {
        let pictures = dirs::picture_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            copy_to_clipboard: true,
            auto_save: true,
            save_dir: pictures.join("Screenshots"),
            check_for_updates: true,
            theme: Theme::System,
            last_update_check: 0,
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("snipshot").join("settings.conf"))
}

impl Settings {
    pub fn load() -> Self {
        let mut settings = Self::default();
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return settings;
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "copy_to_clipboard" => settings.copy_to_clipboard = value == "true",
                "auto_save" => settings.auto_save = value == "true",
                "save_dir" if !value.is_empty() => settings.save_dir = PathBuf::from(value),
                "check_for_updates" => settings.check_for_updates = value == "true",
                "theme" => {
                    settings.theme = Theme::ALL.into_iter().find(|t| t.key() == value).unwrap_or(Theme::System)
                }
                "last_update_check" => settings.last_update_check = value.parse().unwrap_or(0),
                _ => {}
            }
        }
        settings
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = format!(
            "copy_to_clipboard={}\nauto_save={}\nsave_dir={}\ncheck_for_updates={}\ntheme={}\nlast_update_check={}\n",
            self.copy_to_clipboard,
            self.auto_save,
            self.save_dir.display(),
            self.check_for_updates,
            self.theme.key(),
            self.last_update_check
        );
        std::fs::write(path, text)
    }
}
