//! The two interface languages of the app. Text in `.slint` files is translated by
//! Slint's bundled translations; text produced in Rust goes through [`tr`].
use std::sync::atomic::{AtomicBool, Ordering};

static RUSSIAN: AtomicBool = AtomicBool::new(false);

pub fn set_russian(russian: bool) {
    RUSSIAN.store(russian, Ordering::Relaxed);
}

pub fn is_russian() -> bool {
    RUSSIAN.load(Ordering::Relaxed)
}

/// Picks the text for the current language.
pub fn tr(english: &'static str, russian: &'static str) -> &'static str {
    if is_russian() { russian } else { english }
}

/// True when Windows is set to Russian; any other language falls back to English.
#[cfg(windows)]
pub fn system_is_russian() -> bool {
    const LANG_RUSSIAN: u16 = 0x19;
    let language = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    language & 0x3ff == LANG_RUSSIAN
}

#[cfg(not(windows))]
pub fn system_is_russian() -> bool {
    std::env::var("LANG").is_ok_and(|lang| lang.starts_with("ru"))
}
