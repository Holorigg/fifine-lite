//! App preferences stored in %APPDATA%, plus the Windows autostart entry.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// Number of LEDs on the ring.
pub const LED_SLOTS: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub close_action: CloseAction,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    /// Lighting: 0 built-in effect, 1 custom colour, 2 animation.
    #[serde(default)]
    pub light_mode: u8,
    /// Older settings files stored only "custom colour on/off".
    #[serde(default, skip_serializing)]
    custom_color_mode: bool,
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
    /// User labels for the 15 built-in effects; empty means "Эффект N".
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

impl Default for AppSettings {
    fn default() -> Self {
        serde_json::from_str("{}").expect("defaults")
    }
}

impl AppSettings {
    /// Per-LED colours, padded with the primary colour.
    pub fn led_colors(&self) -> Vec<[f32; 3]> {
        let mut leds = self.leds.clone();
        leds.resize(LED_SLOTS, self.custom_color);
        leds
    }
}

fn settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("fifine-am8-lite").join("settings.json"))
}

impl AppSettings {
    pub fn load() -> Self {
        settings_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .map(|mut settings| {
                if settings.custom_color_mode && settings.light_mode == 0 {
                    settings.light_mode = 1;
                }
                settings
            })
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = settings_path().ok_or("APPDATA не задан")?;
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
    const VALUE: &str = "FIFINE AM8 Pro Lite";

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
        Err("Автозапуск поддерживается только в Windows".into())
    }
}

pub use autostart_impl::{is_enabled as autostart_enabled, set as set_autostart};
