//! Notification-area icon: click to open, menu to mute or quit.
use crate::AppWindow;
use slint::{ComponentHandle, Weak};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

pub struct Tray {
    icon: TrayIcon,
    kept: std::cell::RefCell<Vec<Box<dyn std::any::Any>>>,
}

impl Tray {
    pub fn new(ui: Weak<AppWindow>) -> Result<Self, Box<dyn std::error::Error>> {
        let open = MenuItem::new("Открыть", true, None);
        let mute = MenuItem::new("Вкл / выкл микрофон", true, None);
        let quit = MenuItem::new("Выход", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &mute, &PredefinedMenuItem::separator(), &quit])?;

        let icon = TrayIconBuilder::new()
            .with_tooltip("FIFINE AM8 Pro Lite")
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
                    let muted = !ui.get_mic_muted();
                    ui.set_mic_muted(muted);
                    ui.invoke_setting_changed("mic-mute".into(), i32::from(muted));
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

        Ok(Self { icon, kept: Default::default() })
    }

    /// Keeps a timer (or anything else) alive for as long as the tray exists.
    pub fn keep<T: 'static>(&self, value: T) {
        self.kept.borrow_mut().push(Box::new(value));
    }

    /// Swaps the tray icon and tooltip to match the microphone mute state.
    /// Resource 1 is the normal icon, resource 2 the muted one (see build.rs).
    pub fn set_muted(&self, muted: bool) {
        if let Ok(icon) = Icon::from_resource(if muted { 2 } else { 1 }, None) {
            let _ = self.icon.set_icon(Some(icon));
        }
        let _ = self.icon.set_tooltip(Some(if muted {
            "FIFINE AM8 Pro — микрофон выключен"
        } else {
            "FIFINE AM8 Pro — микрофон включён"
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
