#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();
mod app_settings;
mod eq_ui;
mod single_instance;
mod tray;

use app_settings::{AppSettings, CloseAction, LED_SLOTS, TRAY_FLAG};
use fifine_am8_lite::device::{WriteCommand, spawn_writer};
use fifine_am8_lite::editor::EqEditor;
use fifine_am8_lite::helper::{ReaderEvent, spawn_reader};
use fifine_am8_lite::level::{Consumer, LevelMonitor};
use fifine_am8_lite::light::{LightMode, hsv_to_rgb};
use fifine_am8_lite::protocol::Setting;
use slint::{CloseRequestResponse, ComponentHandle, Model};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

/// Set while a screenshot preview runs: the app then never writes lighting to the device.
static PREVIEW: AtomicBool = AtomicBool::new(false);

const EFFECT_COUNT: usize = 15;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A second launch (for example from the Start menu while autostarted) just shows the first copy.
    let Some(instance) = single_instance::Instance::acquire() else {
        return Ok(());
    };
    PREVIEW.store(std::env::var_os("AM8_LITE_PREVIEW").is_some(), Ordering::Relaxed);
    let start_in_tray = std::env::args().any(|arg| arg == TRAY_FLAG);
    let ui = AppWindow::new()?;
    ui.set_device_status("Подключение к AM8 Pro…".into());

    let level = LevelMonitor::spawn();
    let error_ui = ui.as_weak();
    let writer = spawn_writer(level.clone(), move |message| {
        let weak = error_ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_error_message(message.into());
            }
        });
    });

    let setting_writer = writer.clone();
    let config_write_at = Arc::new(Mutex::new(None::<Instant>));
    let setting_write_at = config_write_at.clone();
    let setting_ui = ui.as_weak();
    ui.on_setting_changed(move |name, value| {
        let Ok(value) = u8::try_from(value) else {
            return;
        };
        let setting = match name.as_str() {
            "light-switch" => Setting::LightSwitch,
            "noise-switch" => Setting::NoiseSwitch,
            "mic-mute" => Setting::MicMute,
            "voice-changer" => Setting::VoiceChanger,
            "light-pattern" => Setting::LightPattern,
            "input-volume" => Setting::InputVolume,
            "headphone-volume" => Setting::HeadphoneVolume,
            "monitor-mix" => Setting::MonitorMix,
            _ => return,
        };
        if matches!(setting, Setting::LightSwitch) && value == 0 {
            let _ = setting_writer.send(WriteCommand::Light(None));
        }
        if setting_writer.send(WriteCommand::Setting(setting, value)).is_ok() {
            *setting_write_at.lock().unwrap() = Some(Instant::now());
        }
        // Turning the light back on resumes a custom colour or animation.
        if matches!(setting, Setting::LightSwitch) && value == 1 {
            if let Some(ui) = setting_ui.upgrade() {
                if ui.get_light_mode() != 0 {
                    ui.invoke_light_config_changed();
                }
            }
        }
    });
    let reset_writer = writer.clone();
    ui.on_reset_confirmed(move || {
        let _ = reset_writer.send(WriteCommand::Reset);
    });

    let editor = Arc::new(Mutex::new(EqEditor::new()?));
    let lifecycle_writer = writer.clone();
    eq_ui::wire(&ui, editor.clone(), writer);
    let running = Arc::new(AtomicBool::new(true));
    let weak = ui.as_weak();
    let reader = spawn_reader(running.clone(), move |event| {
        let weak = weak.clone();
        let editor = editor.clone();
        let config_write_at = config_write_at.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            match event {
                ReaderEvent::Connected => ui.set_device_status("Ожидание ответа AM8 Pro…".into()),
                ReaderEvent::Disconnected(message) => {
                    ui.set_device_ready(false);
                    ui.set_eq_ready(false);
                    ui.set_device_status(message.into());
                }
                ReaderEvent::State(state) => {
                    let config = state.configuration;
                    let just_connected = !ui.get_device_ready();
                    ui.set_device_ready(true);
                    ui.set_device_status("AM8 Pro подключён".into());
                    if !config_write_at
                        .lock()
                        .unwrap()
                        .is_some_and(|time| time.elapsed() < Duration::from_millis(1200))
                    {
                        ui.set_light_on(config.light_on);
                        ui.set_noise_on(config.noise_reduction);
                        ui.set_mic_muted(config.mic_mute);
                        ui.set_light_pattern_index(i32::from(config.light_pattern.saturating_sub(1)));
                        ui.set_voice_index(if config.voice_changer < 2 {
                            0
                        } else {
                            i32::from(config.voice_changer - 1)
                        });
                        ui.set_input_volume(f32::from(config.input_volume));
                        ui.set_headphone_volume(f32::from(config.headphone_volume));
                        ui.set_monitor_mix(f32::from(config.monitor_mix));
                    }
                    // Custom lighting lives only while frames stream; restart it on connect.
                    if just_connected && ui.get_light_mode() != 0 {
                        ui.invoke_light_config_changed();
                    }
                    let mut editor = editor.lock().unwrap();
                    if editor.receive_device(state.channel, state.eq_points) {
                        eq_ui::refresh(&ui, &editor);
                    } else {
                        ui.set_eq_ready(editor.loaded());
                    }
                }
            }
        });
    });

    let _tray = wire_window_lifecycle(&ui, instance, lifecycle_writer, level)?;
    apply_preview(&ui);
    if !start_in_tray {
        ui.show()?;
    }
    // Keep running while the window is hidden in the tray; quit only on an explicit exit.
    let result = slint::run_event_loop_until_quit();
    running.store(false, Ordering::Relaxed);
    let _ = reader.join();
    result?;
    Ok(())
}

/// Close-button behaviour, tray icon, autostart, lighting, level meter and the
/// "show me" signal from a second launch.
fn wire_window_lifecycle(
    ui: &AppWindow,
    instance: single_instance::Instance,
    writer: Sender<WriteCommand>,
    level: LevelMonitor,
) -> Result<Rc<tray::Tray>, Box<dyn std::error::Error>> {
    let settings = Rc::new(RefCell::new(AppSettings::load()));
    ui.set_close_action(settings.borrow().close_action.index());
    ui.set_autostart(app_settings::autostart_enabled());
    ui.set_sidebar_collapsed(settings.borrow().sidebar_collapsed);
    let lighting_timer = wire_lighting(ui, &settings, writer);

    let sidebar_settings = settings.clone();
    ui.on_sidebar_toggled(move |collapsed| {
        let mut settings = sidebar_settings.borrow_mut();
        settings.sidebar_collapsed = collapsed;
        let _ = settings.save();
    });

    let weak = ui.as_weak();
    ui.on_autostart_toggled(move |enabled| {
        let Some(ui) = weak.upgrade() else { return };
        if let Err(error) = app_settings::set_autostart(enabled) {
            ui.set_error_message(format!("Не удалось изменить автозапуск: {error}").into());
        }
        ui.set_autostart(app_settings::autostart_enabled());
    });

    let weak = ui.as_weak();
    let close_settings = settings.clone();
    ui.on_close_action_changed(move |index| {
        let mut settings = close_settings.borrow_mut();
        settings.close_action = CloseAction::from_index(index);
        if let (Err(error), Some(ui)) = (settings.save(), weak.upgrade()) {
            ui.set_error_message(format!("Не удалось сохранить настройки: {error}").into());
        }
    });

    let weak = ui.as_weak();
    let choice_settings = settings.clone();
    ui.on_close_choice(move |to_tray, remember| {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_close_prompt(false);
        if remember {
            let action = if to_tray { CloseAction::Tray } else { CloseAction::Exit };
            let mut settings = choice_settings.borrow_mut();
            settings.close_action = action;
            let _ = settings.save();
            ui.set_close_action(action.index());
        }
        if to_tray {
            let _ = ui.hide();
        } else {
            let _ = slint::quit_event_loop();
        }
    });

    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        let Some(ui) = weak.upgrade() else {
            return CloseRequestResponse::HideWindow;
        };
        match settings.borrow().close_action {
            CloseAction::Ask => {
                ui.set_close_prompt(true);
                CloseRequestResponse::KeepWindowShown
            }
            CloseAction::Tray => CloseRequestResponse::HideWindow,
            CloseAction::Exit => {
                let _ = slint::quit_event_loop();
                CloseRequestResponse::HideWindow
            }
        }
    });

    let tray = Rc::new(tray::Tray::new(ui.as_weak())?);
    tray.keep(lighting_timer);
    tray.set_muted(ui.get_mic_muted());
    let mute_tray = tray.clone();
    ui.on_mute_state_changed(move |muted| mute_tray.set_muted(muted));

    // A second launch sets a named event; check it a few times a second.
    let weak = ui.as_weak();
    let show_timer = slint::Timer::default();
    show_timer.start(slint::TimerMode::Repeated, Duration::from_millis(400), move || {
        if instance.show_requested() {
            if let Some(ui) = weak.upgrade() {
                tray::show_window(&ui);
            }
        }
    });
    tray.keep(show_timer);
    tray.keep(wire_level_meter(ui, level));
    Ok(tray)
}

/// Feeds the mixer's level meter. Capture is requested only while the mixer page is
/// on screen in the foreground, so Windows' "microphone in use" mark appears only then.
fn wire_level_meter(ui: &AppWindow, level: LevelMonitor) -> slint::Timer {
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
        let Some(ui) = weak.upgrade() else { return };
        let wanted = ui.get_page() == 0
            && !ui.window().is_minimized()
            && app_in_foreground()
            && !PREVIEW.load(Ordering::Relaxed);
        level.set_wanted(Consumer::Meter, wanted);
        let active = wanted && level.capturing();
        if ui.get_level_active() != active {
            ui.set_level_active(active);
        }
        if active {
            let db = level.peak_db();
            ui.set_mic_level_db(db);
            ui.set_mic_level(((db + 60.0) / 60.0).clamp(0.0, 1.0));
        }
    });
    timer
}

/// Developer preview hook for screenshots: AM8_LITE_PREVIEW=page:N, close, reset, collapsed,
/// color or anim. Applied after saved settings so it wins over them; it never writes to the device.
fn apply_preview(ui: &AppWindow) {
    let Ok(preview) = std::env::var("AM8_LITE_PREVIEW") else { return };
    match preview.as_str() {
        "close" => ui.set_close_prompt(true),
        "reset" => ui.set_confirm_reset(true),
        "collapsed" => ui.set_sidebar_collapsed(true),
        "color" => {
            ui.set_page(2);
            ui.set_light_mode(1);
            ui.set_per_led(true);
        }
        "anim" => {
            ui.set_page(2);
            ui.set_light_mode(2);
            ui.set_anim_index(2);
        }
        other => {
            if let Some(page) = other.strip_prefix("page:").and_then(|n| n.parse().ok()) {
                ui.set_page(page);
            }
        }
    }
}

fn to_slint_color(hsv: [f32; 3]) -> slint::Color {
    let [r, g, b] = hsv_to_rgb(hsv);
    slint::Color::from_rgb_u8(r, g, b)
}

/// Lighting page: effect names, built-in effects, custom colours and animations.
/// Returns the settings-save debounce timer, which must stay alive.
fn wire_lighting(
    ui: &AppWindow,
    settings: &Rc<RefCell<AppSettings>>,
    writer: Sender<WriteCommand>,
) -> Rc<slint::Timer> {
    let names: Vec<slint::SharedString> = {
        let current = settings.borrow();
        (0..EFFECT_COUNT)
            .map(|i| match current.effect_names.get(i).map(|n| n.trim()) {
                Some(name) if !name.is_empty() => name.into(),
                _ => format!("Эффект {}", i + 1).into(),
            })
            .collect()
    };
    ui.set_effect_names(slint::ModelRc::new(slint::VecModel::from(names)));

    let leds = Rc::new(RefCell::new(settings.borrow().led_colors()));
    let led_model = Rc::new(slint::VecModel::from(
        leds.borrow().iter().copied().map(to_slint_color).collect::<Vec<_>>(),
    ));
    ui.set_led_colors(slint::ModelRc::from(led_model.clone()));
    {
        let current = settings.borrow();
        ui.set_light_mode(i32::from(current.light_mode));
        ui.set_per_led(current.per_led);
        ui.set_anim_index(i32::from(current.animation));
        ui.set_anim_speed(current.anim_speed);
        ui.set_second_hue(current.second_hue);
        let [hue, sat, value] = current.custom_color;
        ui.set_custom_hue(hue);
        ui.set_custom_sat(sat);
        ui.set_custom_value(value);
    }

    let weak = ui.as_weak();
    let name_settings = settings.clone();
    ui.on_effect_name_edited(move |index, text| {
        let Ok(index) = usize::try_from(index) else { return };
        let mut settings = name_settings.borrow_mut();
        settings.effect_names.resize(EFFECT_COUNT, String::new());
        if let Some(slot) = settings.effect_names.get_mut(index) {
            *slot = text.trim().to_owned();
        }
        let _ = settings.save();
        if let Some(ui) = weak.upgrade() {
            // Update only the edited entry so the text field keeps its cursor.
            let model = ui.get_effect_names();
            if let Some(vec) = model.as_any().downcast_ref::<slint::VecModel<slint::SharedString>>() {
                if index < vec.row_count() {
                    let label = if text.trim().is_empty() {
                        format!("Эффект {}", index + 1)
                    } else {
                        text.trim().to_owned()
                    };
                    vec.set_row_data(index, label.into());
                }
            }
        }
    });

    // Slider drags fire many changes; save settings once things settle.
    let save_timer = Rc::new(slint::Timer::default());
    let previous_mode = Rc::new(Cell::new(ui.get_light_mode()));

    let weak = ui.as_weak();
    let config_settings = settings.clone();
    let config_leds = leds.clone();
    let config_save = save_timer.clone();
    ui.on_light_config_changed(move || {
        let Some(ui) = weak.upgrade() else { return };
        let primary = [ui.get_custom_hue(), ui.get_custom_sat(), ui.get_custom_value()];
        {
            let mut settings = config_settings.borrow_mut();
            settings.light_mode = ui.get_light_mode().clamp(0, 2) as u8;
            settings.per_led = ui.get_per_led();
            settings.animation = ui.get_anim_index().clamp(0, 3) as u8;
            settings.anim_speed = ui.get_anim_speed();
            settings.second_hue = ui.get_second_hue();
            settings.custom_color = primary;
            settings.leds = config_leds.borrow().clone();
        }
        let save_settings = config_settings.clone();
        config_save.start(slint::TimerMode::SingleShot, Duration::from_millis(400), move || {
            let _ = save_settings.borrow().save();
        });
        if PREVIEW.load(Ordering::Relaxed) {
            return;
        }

        let mode = ui.get_light_mode();
        let was = previous_mode.replace(mode);
        if mode == 0 {
            let _ = writer.send(WriteCommand::Light(None));
            if was != 0 && ui.get_light_on() {
                // Hand the ring back to the selected built-in effect.
                ui.invoke_setting_changed("light-pattern".into(), ui.get_light_pattern_index() + 1);
            }
            return;
        }
        if !ui.get_light_on() || !ui.get_device_ready() {
            let _ = writer.send(WriteCommand::Light(None));
            return;
        }
        let color = hsv_to_rgb(primary);
        let speed = ui.get_anim_speed();
        let light = if mode == 1 {
            if ui.get_per_led() {
                let leds = config_leds.borrow();
                LightMode::Static(std::array::from_fn(|i| hsv_to_rgb(leds[i])))
            } else {
                LightMode::Static([color; LED_SLOTS])
            }
        } else {
            match ui.get_anim_index() {
                0 => LightMode::Breath { color, speed },
                1 => LightMode::Rainbow { speed },
                2 => LightMode::Gradient { from: color, to: hsv_to_rgb([ui.get_second_hue(), 0.85, 1.0]), speed },
                _ => LightMode::Voice { color },
            }
        };
        let _ = writer.send(WriteCommand::Light(Some(light)));
    });

    let weak = ui.as_weak();
    let paint_leds = leds.clone();
    let paint_model = led_model.clone();
    ui.on_led_painted(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let Ok(index) = usize::try_from(index) else { return };
        let hsv = [ui.get_custom_hue(), ui.get_custom_sat(), ui.get_custom_value()];
        if let Some(slot) = paint_leds.borrow_mut().get_mut(index) {
            *slot = hsv;
            paint_model.set_row_data(index, to_slint_color(hsv));
        }
        ui.invoke_light_config_changed();
    });

    let weak = ui.as_weak();
    ui.on_led_fill_all(move || {
        let Some(ui) = weak.upgrade() else { return };
        let hsv = [ui.get_custom_hue(), ui.get_custom_sat(), ui.get_custom_value()];
        let mut leds = leds.borrow_mut();
        for (index, slot) in leds.iter_mut().enumerate() {
            *slot = hsv;
            led_model.set_row_data(index, to_slint_color(hsv));
        }
        drop(leds);
        ui.invoke_light_config_changed();
    });
    save_timer
}

/// True when the foreground window belongs to this process.
#[cfg(windows)]
fn app_in_foreground() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut pid = 0u32;
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return false;
        }
        GetWindowThreadProcessId(window, &mut pid);
    }
    pid == std::process::id()
}

#[cfg(not(windows))]
fn app_in_foreground() -> bool {
    true
}
