//! Notification-area icon: click to open, menu to mute or quit.
use crate::AppWindow;
use fifine_am8_lite::i18n::tr;
use slint::{ComponentHandle, Weak};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

pub struct Tray {
    icon: TrayIcon,
    open: MenuItem,
    mute: MenuItem,
    quit: MenuItem,
    muted: std::cell::Cell<bool>,
    kept: std::cell::RefCell<Vec<Box<dyn std::any::Any>>>,
}

impl Tray {
    pub fn new(ui: Weak<AppWindow>) -> Result<Self, Box<dyn std::error::Error>> {
        let open = MenuItem::new("", true, None);
        let mute = MenuItem::new("", true, None);
        let quit = MenuItem::new("", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &mute, &PredefinedMenuItem::separator(), &quit])?;

        let icon = TrayIconBuilder::new()
            .with_icon(Icon::from_resource(1, None)?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        let (open_id, mute_id, quit_id) = (open.id().clone(), mute.id().clone(), quit.id().clone());
        let menu_ui = ui.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let (ui, id) = (menu_ui.clone(), event.id);
            let (open_id, mute_id, quit_id) = (open_id.clone(), mute_id.clone(), quit_id.clone());
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui.upgrade() else { return };
                if id == open_id {
                    show_window(&ui);
                } else if id == mute_id {
                    if ui.get_device_ready() && ui.get_has_mute() {
                        let muted = !ui.get_mic_muted();
                        ui.set_mic_muted(muted);
                        ui.invoke_setting_changed("mic-mute".into(), i32::from(muted));
                    }
                } else if id == quit_id {
                    let _ = slint::quit_event_loop();
                }
            });
        }));

        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                let ui = ui.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui.upgrade() {
                        show_window(&ui);
                    }
                });
            }
        }));

        let tray = Self { icon, open, mute, quit, muted: Default::default(), kept: Default::default() };
        tray.retranslate();
        Ok(tray)
    }

    /// Keeps a timer (or anything else) alive for as long as the tray exists.
    pub fn keep<T: 'static>(&self, value: T) {
        self.kept.borrow_mut().push(Box::new(value));
    }

    /// Puts the menu and tooltip into the current language.
    pub fn retranslate(&self) {
        self.open.set_text(tr("Open", "Открыть"));
        self.mute.set_text(tr("Mute / unmute microphone", "Вкл / выкл микрофон"));
        self.quit.set_text(tr("Quit", "Выход"));
        self.set_muted(self.muted.get());
    }

    /// Swaps the tray icon and tooltip to match the microphone mute state.
    /// Resource 1 is the normal icon, resource 2 the muted one (see build.rs).
    pub fn set_muted(&self, muted: bool) {
        self.muted.set(muted);
        if let Ok(icon) = Icon::from_resource(if muted { 2 } else { 1 }, None) {
            let _ = self.icon.set_icon(Some(icon));
        }
        let _ = self.icon.set_tooltip(Some(if muted {
            tr("FIFINE Lite — microphone muted", "FIFINE Lite — микрофон выключен")
        } else {
            tr("FIFINE Lite — microphone on", "FIFINE Lite — микрофон включён")
        }));
    }
}

/// Shows, restores and focuses the main window.
pub fn show_window(ui: &AppWindow) {
    let _ = ui.show();
    ui.window().set_minimized(false);
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = ui.window().window_handle().window_handle() {
            if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(win32.hwnd.get() as _);
                }
            }
        }
    }
}
