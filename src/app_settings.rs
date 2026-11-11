//! App preferences stored in %APPDATA%, plus the Windows autostart entry.
use fifine_lite::profile::MAX_LEDS;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// While set, `save` does nothing (screenshot previews must not touch real settings).
pub static READ_ONLY: AtomicBool = AtomicBool::new(false);

/// What the window close button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CloseAction {
    #[default]
    Ask,
    Tray,
    Exit,
}

impl CloseAction {
    pub fn index(self) -> i32 {
        match self {
            CloseAction::Ask => 0,
            CloseAction::Tray => 1,
            CloseAction::Exit => 2,
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            1 => CloseAction::Tray,
            2 => CloseAction::Exit,
            _ => CloseAction::Ask,
        }
    }
}

/// Interface language. `Auto` follows Windows: Russian there, English everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Language {
    #[default]
    Auto,
    English,
    Russian,
}

impl Language {
    pub fn index(self) -> i32 {
        match self {
            Language::Auto => 0,
            Language::English => 1,
            Language::Russian => 2,
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Language::English,
            2 => Language::Russian,
            _ => Language::Auto,
        }
    }

    pub fn is_russian(self) -> bool {
        match self {
            Language::Auto => fifine_lite::i18n::system_is_russian(),
            Language::English => false,
            Language::Russian => true,
        }
    }
}

/// Lighting choices; every microphone model keeps its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightSettings {
    /// 0 built-in effect, 1 custom colour, 2 animation.
    #[serde(default)]
    pub light_mode: u8,
    /// Primary custom colour as hue (0–360), saturation and brightness (0–1).
    #[serde(default = "default_custom_color")]
    pub custom_color: [f32; 3],
    /// Paint each LED separately instead of one colour.
    #[serde(default)]
    pub per_led: bool,
    /// Per-LED colours (HSV), used when `per_led` is on.
    #[serde(default)]
    pub leds: Vec<[f32; 3]>,
    /// 0 breath, 1 rainbow, 2 gradient, 3 voice.
    #[serde(default)]
    pub animation: u8,
    #[serde(default = "default_speed")]
    pub anim_speed: f32,
    /// Second colour of the gradient animation (hue).
    #[serde(default = "default_second_hue")]
    pub second_hue: f32,
    /// Built-in effect (counted from 0) to return to when custom lighting is turned off.
    #[serde(default)]
    pub builtin_pattern: u8,
    /// User labels for the built-in effects; empty means "Effect N".
    #[serde(default)]
    pub effect_names: Vec<String>,
}

fn default_custom_color() -> [f32; 3] {
    [260.0, 0.65, 1.0]
}
fn default_speed() -> f32 {
    1.0
}
fn default_second_hue() -> f32 {
    190.0
}

impl Default for LightSettings {
    fn default() -> Self {
        serde_json::from_str("{}").expect("defaults")
    }
}

impl LightSettings {
    /// Per-LED colours, padded with the primary colour.
    pub fn led_colors(&self) -> Vec<[f32; 3]> {
        let mut leds = self.leds.clone();
        leds.resize(MAX_LEDS, self.custom_color);
        leds
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub close_action: CloseAction,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub language: Language,
    /// Model used last; preferred when several microphones are plugged in.
    #[serde(default)]
    pub last_model: String,
    /// Lighting per model, keyed by profile id.
    #[serde(default)]
    pub devices: BTreeMap<String, LightSettings>,
}

fn settings_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join(name))
}

fn settings_path() -> Option<PathBuf> {
    settings_dir("fifine-lite").map(|dir| dir.join("settings.json"))
}

/// Where versions before 0.3.1 (named "FIFINE AM8 Pro Lite") kept their settings.
fn old_settings_path() -> Option<PathBuf> {
    settings_dir("fifine-am8-lite").map(|dir| dir.join("settings.json"))
}

impl AppSettings {
    pub fn load() -> Self {
        [settings_path(), old_settings_path()]
            .into_iter()
            .flatten()
            .find_map(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
        let mut settings: Self = serde_json::from_value(value.clone()).ok()?;
        // Versions before 0.3 knew only the AM8 Pro and kept its lighting at the top level.
        if settings.devices.is_empty() && value.get("light_mode").is_some() {
            if let Ok(mut light) = serde_json::from_value::<LightSettings>(value.clone()) {
                if value.get("custom_color_mode").and_then(|v| v.as_bool()) == Some(true) && light.light_mode == 0 {
                    light.light_mode = 1;
                }
                settings.devices.insert("am8pro".to_owned(), light);
            }
        }
        Some(settings)
    }

    pub fn light(&self, model: &str) -> LightSettings {
        self.devices.get(model).cloned().unwrap_or_default()
    }

    pub fn light_mut(&mut self, model: &str) -> &mut LightSettings {
        self.devices.entry(model.to_owned()).or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        if READ_ONLY.load(Ordering::Relaxed) {
            return Ok(());
        }
        let path = settings_path().ok_or("APPDATA is not set")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
}

/// Command-line flag used by the autostart entry: start hidden in the tray.
pub const TRAY_FLAG: &str = "--tray";

#[cfg(windows)]
mod autostart_impl {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "FIFINE Lite";
    /// Name of the autostart entry before the app was renamed.
    const OLD_VALUE: &str = "FIFINE AM8 Pro Lite";

    fn command() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Ok(format!("\"{}\" {}", exe.display(), super::TRAY_FLAG))
    }

    pub fn is_enabled() -> bool {
        let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(RUN_KEY) else {
            return false;
        };
        let Ok(value) = key.get_value::<String, _>(VALUE) else {
            return false;
        };
        // Enabled only if it points at this executable.
        command().is_ok_and(|expected| value.eq_ignore_ascii_case(&expected))
    }

    /// Replaces an autostart entry left by a version with the old name, so the renamed
    /// app keeps starting with Windows.
    pub fn migrate() {
        let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_ALL_ACCESS)
        else {
            return;
        };
        if key.get_value::<String, _>(OLD_VALUE).is_ok() && set(true).is_ok() {
            let _ = key.delete_value(OLD_VALUE);
        }
    }

    pub fn set(enabled: bool) -> Result<(), String> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(RUN_KEY)
            .map_err(|e| e.to_string())?;
        if enabled {
            key.set_value(VALUE, &command()?).map_err(|e| e.to_string())
        } else {
            match key.delete_value(VALUE) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(not(windows))]
mod autostart_impl {
    pub fn is_enabled() -> bool {
        false
    }
    pub fn set(_: bool) -> Result<(), String> {
        Err("autostart is only supported on Windows".into())
    }
    pub fn migrate() {}
}

pub use autostart_impl::{is_enabled as autostart_enabled, migrate as migrate_autostart, set as set_autostart};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_flat_settings_become_the_am8_pro_entry() {
        let old = r#"{"close_action":"Tray","light_mode":2,"animation":1,"builtin_pattern":8,"effect_names":["Wave"]}"#;
        let settings = AppSettings::parse(old).unwrap();
        assert_eq!(settings.close_action, CloseAction::Tray);
        let light = settings.light("am8pro");
        assert_eq!((light.light_mode, light.animation, light.builtin_pattern), (2, 1, 8));
        assert_eq!(light.effect_names, ["Wave"]);
        assert_eq!(settings.light("am9").light_mode, 0);
    }
}
