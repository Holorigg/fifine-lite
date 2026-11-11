#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();
mod app_settings;
mod eq_ui;
mod single_instance;
mod tray;

use app_settings::{AppSettings, CloseAction, Language, TRAY_FLAG};
use fifine_am8_lite::device::{DeviceEvent, DeviceInfo, Problem, WriteCommand, spawn_device};
use fifine_am8_lite::editor::EqEditor;
use fifine_am8_lite::i18n::{self, tr};
use fifine_am8_lite::level::{Consumer, LevelMonitor};
use fifine_am8_lite::light::{LightMode, hsv_to_rgb};
use fifine_am8_lite::profile::{self, MAX_LEDS, PROFILES, Profile};
use fifine_am8_lite::protocol::{DeviceState, PATTERN_CUSTOM, Setting};
use slint::{CloseRequestResponse, ComponentHandle, Model, SharedString};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

/// Set while a screenshot preview runs: the app then never writes to the device or to
/// its settings file.
static PREVIEW: AtomicBool = AtomicBool::new(false);

/// After the app changes something, state reports are ignored for this long: the
/// microphone needs a moment before it reports the new value.
const SETTLE: Duration = Duration::from_millis(1200);

type Hsv = [f32; 3];

thread_local! {
    /// Runs on the UI thread for every event from the device thread.
    static ON_DEVICE_EVENT: RefCell<Option<Box<dyn Fn(DeviceEvent)>>> = const { RefCell::new(None) };
}

/// State shared by the handlers; everything here lives on the UI thread.
struct App {
    ui: slint::Weak<AppWindow>,
    writer: Sender<WriteCommand>,
    settings: Rc<RefCell<AppSettings>>,
    editor: Arc<Mutex<EqEditor>>,
    /// Model the controls are laid out for: the connected microphone, or the last one used.
    profile: Cell<&'static Profile>,
    /// Supported microphones that are plugged in.
    devices: RefCell<Vec<DeviceInfo>>,
    /// The user picked a microphone in this run, so the app stops choosing for them.
    picked: Cell<bool>,
    /// Why there is no connection, kept so the banner can follow a language change.
    problem: RefCell<Option<Problem>>,
    /// Per-LED colours of the "paint each LED" mode.
    leds: RefCell<Vec<Hsv>>,
    /// Colours the ring diagram currently shows.
    ring: Rc<slint::VecModel<slint::Color>>,
    last_write: Cell<Option<Instant>>,
    /// A preview of another model is on screen: ignore the real microphone.
    frozen: Cell<bool>,
    level: LevelMonitor,
    tray: RefCell<Option<Rc<tray::Tray>>>,
}

fn problem_text(problem: &Problem) -> String {
    match problem {
        Problem::NotFound => tr(
            "No supported fifine microphone found. Check the USB cable.",
            "Поддерживаемый микрофон fifine не найден. Проверьте USB-кабель.",
        )
        .to_owned(),
        Problem::NotResponding => {
            tr("The microphone is not responding. Check the USB cable.", "Микрофон не отвечает. Проверьте USB-кабель.")
                .to_owned()
        }
        Problem::CannotOpen(error) => {
            format!("{}: {error}", tr("Could not open the microphone", "Не удалось открыть микрофон"))
        }
        Problem::InvalidValue => {
            tr("This microphone does not accept that value", "Микрофон не принимает такое значение").to_owned()
        }
        Problem::InvalidEq => {
            tr("This microphone does not accept that equalizer curve", "Микрофон не принимает такую кривую эквалайзера")
                .to_owned()
        }
    }
}

impl App {
    fn send(&self, command: WriteCommand) {
        if self.writer.send(command).is_ok() {
            self.last_write.set(Some(Instant::now()));
        }
    }

    fn settled(&self) -> bool {
        !self.last_write.get().is_some_and(|time| time.elapsed() < SETTLE)
    }

    fn save_settings(&self) {
        if let (Err(error), Some(ui)) = (self.settings.borrow().save(), self.ui.upgrade()) {
            let text = tr("Could not save settings", "Не удалось сохранить настройки");
            ui.set_error_message(format!("{text}: {error}").into());
        }
    }

    /// The custom lighting chosen in the UI; `None` while a built-in effect is selected.
    fn light_mode(&self, ui: &AppWindow) -> Option<LightMode> {
        let color = hsv_to_rgb([ui.get_custom_hue(), ui.get_custom_sat(), ui.get_custom_value()]);
        let speed = ui.get_anim_speed();
        match ui.get_light_mode() {
            1 if ui.get_per_led() => {
                let leds = self.leds.borrow();
                Some(LightMode::Static(std::array::from_fn(|i| hsv_to_rgb(leds[i]))))
            }
            1 => Some(LightMode::Static([color; MAX_LEDS])),
            2 => Some(match ui.get_anim_index() {
                0 => LightMode::Breath { color, speed },
                1 => LightMode::Rainbow { speed },
                2 => LightMode::Gradient { from: color, to: hsv_to_rgb([ui.get_second_hue(), 0.85, 1.0]), speed },
                _ => LightMode::Voice { color },
            }),
            _ => None,
        }
    }

    /// Shows the colours of the current mode on the ring diagram (a still picture).
    fn update_ring(&self, ui: &AppWindow) {
        let Some(mode) = self.light_mode(ui) else { return };
        let leds = self.profile.get().leds;
        for (index, [r, g, b]) in mode.rest_frame(leds).into_iter().take(leds).enumerate() {
            let color = slint::Color::from_rgb_u8(r, g, b);
            if self.ring.row_data(index) != Some(color) {
                self.ring.set_row_data(index, color);
            }
        }
    }

    /// Labels of the built-in effects: the user's own, or "Effect N".
    fn show_effect_names(&self, ui: &AppWindow) {
        let profile = self.profile.get();
        let light = self.settings.borrow().light(profile.id);
        let custom: Vec<SharedString> = (0..usize::from(profile.patterns))
            .map(|i| light.effect_names.get(i).map(|name| name.trim()).unwrap_or_default().into())
            .collect();
        let labels: Vec<SharedString> = custom
            .iter()
            .enumerate()
            .map(|(i, name)| if name.is_empty() { format!("{} {}", tr("Effect", "Эффект"), i + 1).into() } else { name.clone() })
            .collect();
        ui.set_effect_names(slint::ModelRc::new(slint::VecModel::from(labels)));
        ui.set_effect_custom_names(slint::ModelRc::new(slint::VecModel::from(custom)));
    }

    /// Lays the controls out for a model and loads that model's lighting choices.
    fn show_profile(&self, ui: &AppWindow, profile: &'static Profile) {
        self.profile.set(profile);
        self.level.set_model(profile.name);
        let (codes, layout) = (&profile.codes, profile.layout.as_ref());
        ui.set_device_name(profile.name.into());
        ui.set_experimental(!profile.verified);
        ui.set_has_image(profile.id == "am8pro");
        ui.set_has_light_switch(codes.light_switch.is_some());
        ui.set_has_custom_light(profile.leds > 0);
        ui.set_pattern_count(i32::from(profile.patterns));
        ui.set_has_mute(codes.mute.is_some());
        ui.set_noise_levels(if codes.noise.is_some() { i32::from(profile.noise_levels) } else { 0 });
        ui.set_has_voice(codes.voice.is_some());
        ui.set_has_input(codes.input_volume.is_some());
        ui.set_has_headphone(codes.headphone_volume.is_some());
        ui.set_has_mix(codes.monitor_mix.is_some());
        ui.set_has_pickup(codes.pickup.is_some());
        ui.set_has_low_cut(codes.low_cut.is_some());
        ui.set_has_mid_boost(codes.mid_boost.is_some());
        ui.set_has_eq(profile.has_eq);
        ui.set_has_reset(profile.has_reset);
        ui.set_volume_max(f32::from(profile.volume_max.max(1)));
        ui.set_mix_max(f32::from(profile.mix_max.max(1)));
        // A microphone without a readable light switch is treated as always lit.
        if layout.is_none_or(|layout| layout.light_switch.is_none()) {
            ui.set_light_on(true);
        }
        let page = ui.get_page();
        if (page == 1 && !profile.has_eq) || (page == 2 && profile.leds == 0 && profile.patterns == 0) {
            ui.set_page(0);
        }

        let light = self.settings.borrow().light(profile.id);
        *self.leds.borrow_mut() = light.led_colors();
        self.ring.set_vec(vec![slint::Color::default(); profile.leds]);
        ui.set_light_mode(i32::from(light.light_mode.min(2)));
        ui.set_per_led(light.per_led);
        ui.set_anim_index(i32::from(light.animation.min(3)));
        ui.set_anim_speed(light.anim_speed.clamp(0.2, 3.0));
        ui.set_second_hue(light.second_hue);
        ui.set_light_pattern_index(i32::from(light.builtin_pattern.min(profile.patterns.saturating_sub(1))));
        let [hue, sat, value] = light.custom_color;
        ui.set_custom_hue(hue);
        ui.set_custom_sat(sat);
        ui.set_custom_value(value);
        self.show_effect_names(ui);
        self.update_ring(ui);
    }

    /// Names for the microphone picker; two of the same model get a number.
    fn show_devices(&self, ui: &AppWindow) {
        let devices = self.devices.borrow();
        let names: Vec<SharedString> = devices
            .iter()
            .enumerate()
            .map(|(index, device)| {
                let same = |other: &&DeviceInfo| other.profile == device.profile;
                if devices.iter().filter(same).count() > 1 {
                    let number = devices[..=index].iter().filter(same).count();
                    format!("{} ({number})", device.profile.name).into()
                } else {
                    device.profile.name.into()
                }
            })
            .collect();
        ui.set_device_names(slint::ModelRc::new(slint::VecModel::from(names)));
    }

    fn on_state(&self, ui: &AppWindow, state: DeviceState) {
        let profile = self.profile.get();
        let just_connected = !ui.get_device_ready();
        ui.set_device_ready(true);
        *self.problem.borrow_mut() = None;
        if self.settled() {
            if let Some(on) = state.light_on {
                ui.set_light_on(on);
            }
            if let Some(level) = state.noise {
                ui.set_noise_level(i32::from(level.min(profile.noise_levels)));
            }
            if let Some(muted) = state.mic_mute {
                ui.set_mic_muted(muted);
            }
            if let Some(voice) = state.voice {
                ui.set_voice_index(if voice < 2 { 0 } else { i32::from(voice - 1) });
            }
            if let Some(volume) = state.input_volume {
                ui.set_input_volume(f32::from(volume));
            }
            if let Some(volume) = state.headphone_volume {
                ui.set_headphone_volume(f32::from(volume));
            }
            if let Some(mix) = state.monitor_mix {
                ui.set_monitor_mix(f32::from(mix));
            }
            if let Some(pickup) = state.pickup {
                ui.set_pickup_index(i32::from(pickup.clamp(1, 4) - 1));
            }
            if let Some(on) = state.low_cut {
                ui.set_low_cut(on);
            }
            if let Some(on) = state.mid_boost {
                ui.set_mid_boost(on);
            }
            let lit = ui.get_light_on();
            match state.light_pattern {
                Some(pattern) if (1..=profile.patterns).contains(&pattern) => {
                    let index = pattern - 1;
                    ui.set_light_pattern_index(i32::from(index));
                    if self.settings.borrow().light(profile.id).builtin_pattern != index {
                        self.settings.borrow_mut().light_mut(profile.id).builtin_pattern = index;
                        self.save_settings();
                    }
                    // A built-in effect was chosen with the button on the microphone. Only
                    // models that hold custom colours report this reliably.
                    if profile.holds_custom_light && !just_connected && lit && ui.get_light_mode() != 0 {
                        ui.set_light_mode(0);
                        ui.invoke_light_config_changed();
                    }
                }
                // The ring already shows custom colours, e.g. left there by an earlier run.
                Some(PATTERN_CUSTOM) if profile.holds_custom_light && lit && ui.get_light_mode() == 0 => {
                    ui.set_light_mode(1);
                    ui.invoke_light_config_changed();
                }
                _ => {}
            }
        }
        // After a reconnect the microphone may be back on a built-in effect.
        if just_connected && ui.get_light_mode() != 0 {
            ui.invoke_light_config_changed();
        }
        if let Some((channel, points)) = state.eq {
            let mut editor = self.editor.lock().unwrap();
            if editor.receive_device(channel, points) {
                eq_ui::refresh(ui, &editor);
            } else {
                ui.set_eq_ready(editor.loaded());
            }
        }
    }

    fn on_device_event(&self, event: DeviceEvent) {
        let Some(ui) = self.ui.upgrade() else { return };
        if self.frozen.get() {
            return;
        }
        match event {
            DeviceEvent::Devices(devices) => {
                // Prefer the model used last time until the user picks one themselves.
                if !self.picked.get() {
                    let last = self.settings.borrow().last_model.clone();
                    if let Some(device) = devices.iter().find(|device| device.profile.id == last) {
                        self.send(WriteCommand::Select(device.key.clone()));
                    }
                }
                *self.devices.borrow_mut() = devices;
                self.show_devices(&ui);
            }
            DeviceEvent::Connected(device) => {
                ui.set_device_ready(false);
                ui.set_eq_ready(false);
                self.editor.lock().unwrap().forget_device();
                self.show_profile(&ui, device.profile);
                let index = self.devices.borrow().iter().position(|other| other.key == device.key);
                ui.set_device_index(index.map_or(0, |index| index as i32));
                if self.settings.borrow().last_model != device.profile.id {
                    self.settings.borrow_mut().last_model = device.profile.id.to_owned();
                    self.save_settings();
                }
            }
            DeviceEvent::State(state) => self.on_state(&ui, state),
            DeviceEvent::Disconnected(problem) => {
                ui.set_device_ready(false);
                ui.set_eq_ready(false);
                ui.set_device_status(problem_text(&problem).into());
                *self.problem.borrow_mut() = Some(problem);
                // The microphone may come back with other settings; read the EQ again.
                self.editor.lock().unwrap().forget_device();
            }
            DeviceEvent::Rejected(problem) => ui.set_error_message(problem_text(&problem).into()),
        }
    }

    /// Switches every piece of text to the language chosen in the settings.
    fn apply_language(&self, ui: &AppWindow) {
        let russian = self.settings.borrow().language.is_russian();
        i18n::set_russian(russian);
        let _ = slint::select_bundled_translation(if russian { "ru" } else { "en" });
        self.show_effect_names(ui);
        eq_ui::refresh(ui, &self.editor.lock().unwrap());
        let status = match &*self.problem.borrow() {
            Some(problem) => problem_text(problem),
            None => tr("Looking for a microphone…", "Поиск микрофона…").to_owned(),
        };
        ui.set_device_status(status.into());
        if let Some(tray) = &*self.tray.borrow() {
            tray.retranslate();
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A second launch (for example from the Start menu while autostarted) just shows the first copy.
    let Some(instance) = single_instance::Instance::acquire() else {
        return Ok(());
    };
    let preview = std::env::var_os("AM8_LITE_PREVIEW").is_some();
    PREVIEW.store(preview, Ordering::Relaxed);
    app_settings::READ_ONLY.store(preview, Ordering::Relaxed);
    let start_in_tray = std::env::args().any(|arg| arg == TRAY_FLAG);
    let mut settings = AppSettings::load();
    // AM8_LITE_LANG=en|ru overrides the language for one run (handy for screenshots).
    match std::env::var("AM8_LITE_LANG").as_deref() {
        Ok("en") => settings.language = Language::English,
        Ok("ru") => settings.language = Language::Russian,
        _ => {}
    }
    i18n::set_russian(settings.language.is_russian());
    let ui = AppWindow::new()?;
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());

    let level = LevelMonitor::spawn();
    let (device_writer, device_thread) = spawn_device(level.clone(), |event| {
        let _ = slint::invoke_from_event_loop(move || {
            ON_DEVICE_EVENT.with(|handler| {
                if let Some(handler) = handler.borrow().as_ref() {
                    handler(event);
                }
            });
        });
    });
    // A preview only reads the microphone: its commands go nowhere.
    let writer = if preview {
        let (sender, receiver) = mpsc::channel();
        Box::leak(Box::new(receiver));
        sender
    } else {
        device_writer.clone()
    };

    let last_profile = profile::by_id(&settings.last_model).unwrap_or(&PROFILES[0]);
    let app = Rc::new(App {
        ui: ui.as_weak(),
        writer: writer.clone(),
        settings: Rc::new(RefCell::new(settings)),
        editor: Arc::new(Mutex::new(EqEditor::new()?)),
        profile: Cell::new(last_profile),
        devices: RefCell::default(),
        picked: Cell::new(false),
        problem: RefCell::default(),
        leds: RefCell::default(),
        ring: Rc::new(slint::VecModel::default()),
        last_write: Cell::new(None),
        frozen: Cell::new(false),
        level,
        tray: RefCell::default(),
    });

    ui.set_ring_colors(slint::ModelRc::from(app.ring.clone()));
    app.show_profile(&ui, last_profile);
    wire_settings(&ui, &app);
    eq_ui::wire(&ui, app.editor.clone(), writer);
    wire_lighting(&ui, &app);
    let tray = wire_window_lifecycle(&ui, &app, instance)?;
    *app.tray.borrow_mut() = Some(tray);
    app.apply_language(&ui);
    apply_preview(&ui, &app);

    let event_app = app.clone();
    ON_DEVICE_EVENT.with(|handler| {
        *handler.borrow_mut() = Some(Box::new(move |event| event_app.on_device_event(event)));
    });

    if !start_in_tray {
        ui.show()?;
    }
    // Keep running while the window is hidden in the tray; quit only on an explicit exit.
    let result = slint::run_event_loop_until_quit();

    // An animation cannot outlive the app. Where the microphone keeps custom colours by
    // itself, leave the animation's colours on the ring; elsewhere it returns to its own effect.
    let profile = app.profile.get();
    let rest = (profile.holds_custom_light && ui.get_device_ready() && ui.get_light_on() && ui.get_light_mode() == 2)
        .then(|| app.light_mode(&ui).map(|mode| mode.rest_frame(profile.leds)))
        .flatten();
    let _ = app.writer.send(WriteCommand::Quit(rest));
    let _ = device_writer.send(WriteCommand::Quit(None));
    let _ = device_thread.join();
    result?;
    Ok(())
}

/// Mixer controls, microphone picker, factory reset.
fn wire_settings(ui: &AppWindow, app: &Rc<App>) {
    let state = app.clone();
    ui.on_setting_changed(move |name, value| {
        let Some(ui) = state.ui.upgrade() else { return };
        let Ok(value) = u8::try_from(value) else { return };
        let setting = match name.as_str() {
            "light-switch" => Setting::LightSwitch,
            "light-pattern" => Setting::LightPattern,
            "mic-mute" => Setting::Mute,
            "noise" => Setting::Noise,
            "voice-changer" => Setting::Voice,
            "input-volume" => Setting::InputVolume,
            "headphone-volume" => Setting::HeadphoneVolume,
            "monitor-mix" => Setting::MonitorMix,
            "pickup" => Setting::Pickup,
            "low-cut" => Setting::LowCut,
            "mid-boost" => Setting::MidBoost,
            _ => return,
        };
        match setting {
            // Frames would switch the light straight back on.
            Setting::LightSwitch if value == 0 => state.send(WriteCommand::Light(None)),
            Setting::LightPattern => {
                let model = state.profile.get().id;
                state.settings.borrow_mut().light_mut(model).builtin_pattern = value.saturating_sub(1);
                state.save_settings();
            }
            _ => {}
        }
        state.send(WriteCommand::Setting(setting, value));
        // Turning the light back on resumes a custom colour or animation.
        if matches!(setting, Setting::LightSwitch) && value == 1 && ui.get_light_mode() != 0 {
            ui.invoke_light_config_changed();
        }
    });

    let state = app.clone();
    ui.on_device_selected(move |index| {
        let key = usize::try_from(index).ok().and_then(|index| state.devices.borrow().get(index).map(|d| d.key.clone()));
        if let Some(key) = key {
            state.picked.set(true);
            state.send(WriteCommand::Light(None));
            state.send(WriteCommand::Select(key));
        }
    });

    let state = app.clone();
    ui.on_reset_confirmed(move || {
        let Some(ui) = state.ui.upgrade() else { return };
        state.send(WriteCommand::Light(None));
        state.send(WriteCommand::Reset);
        // The microphone is back on its own lighting and EQ: read both again.
        ui.set_light_mode(0);
        ui.invoke_light_config_changed();
        let mut editor = state.editor.lock().unwrap();
        editor.forget_device();
        eq_ui::refresh(&ui, &editor);
    });
}

/// Close-button behaviour, tray icon, autostart, language, the level meter tick and the
/// "show me" signal from a second launch.
fn wire_window_lifecycle(
    ui: &AppWindow,
    app: &Rc<App>,
    instance: single_instance::Instance,
) -> Result<Rc<tray::Tray>, Box<dyn std::error::Error>> {
    {
        let settings = app.settings.borrow();
        ui.set_close_action(settings.close_action.index());
        ui.set_sidebar_collapsed(settings.sidebar_collapsed);
        ui.set_language_index(settings.language.index());
    }
    ui.set_autostart(app_settings::autostart_enabled());

    let state = app.clone();
    ui.on_sidebar_toggled(move |collapsed| {
        state.settings.borrow_mut().sidebar_collapsed = collapsed;
        state.save_settings();
    });

    let state = app.clone();
    ui.on_language_changed(move |index| {
        let Some(ui) = state.ui.upgrade() else { return };
        state.settings.borrow_mut().language = Language::from_index(index);
        state.save_settings();
        state.apply_language(&ui);
    });

    let weak = ui.as_weak();
    ui.on_autostart_toggled(move |enabled| {
        let Some(ui) = weak.upgrade() else { return };
        if let Err(error) = app_settings::set_autostart(enabled) {
            let text = tr("Could not change autostart", "Не удалось изменить автозапуск");
            ui.set_error_message(format!("{text}: {error}").into());
        }
        ui.set_autostart(app_settings::autostart_enabled());
    });

    let state = app.clone();
    ui.on_close_action_changed(move |index| {
        state.settings.borrow_mut().close_action = CloseAction::from_index(index);
        state.save_settings();
    });

    let state = app.clone();
    ui.on_close_choice(move |to_tray, remember| {
        let Some(ui) = state.ui.upgrade() else { return };
        ui.set_close_prompt(false);
        if remember {
            let action = if to_tray { CloseAction::Tray } else { CloseAction::Exit };
            state.settings.borrow_mut().close_action = action;
            state.save_settings();
            ui.set_close_action(action.index());
        }
        if to_tray {
            let _ = ui.hide();
        } else {
            let _ = slint::quit_event_loop();
        }
    });

    let state = app.clone();
    ui.window().on_close_requested(move || {
        let Some(ui) = state.ui.upgrade() else {
            return CloseRequestResponse::HideWindow;
        };
        let action = state.settings.borrow().close_action;
        match action {
            CloseAction::Ask => {
                ui.set_close_remember(false);
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
    tray.keep(wire_tick(app));
    Ok(tray)
}

/// Feeds the mixer's level meter. Capture is requested only while the mixer page is on
/// screen in the foreground, so Windows' "microphone in use" mark appears only then.
fn wire_tick(app: &Rc<App>) -> slint::Timer {
    let state = app.clone();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
        let Some(ui) = state.ui.upgrade() else { return };
        let on_screen = ui.window().is_visible() && !ui.window().is_minimized();
        let wanted = on_screen
            && ui.get_page() == 0
            && ui.get_device_ready()
            && app_in_foreground()
            && !PREVIEW.load(Ordering::Relaxed);
        state.level.set_wanted(Consumer::Meter, wanted);
        let active = wanted && state.level.capturing();
        if ui.get_level_active() != active {
            ui.set_level_active(active);
        }
        if active {
            let db = state.level.peak_db();
            ui.set_mic_level_db(db);
            ui.set_mic_level(((db + 60.0) / 60.0).clamp(0.0, 1.0));
        }
    });
    timer
}

/// Developer preview hook for screenshots. AM8_LITE_PREVIEW is one of page:N, close, reset,
/// collapsed, effects, color, anim, or model:<id> (lays the window out for that model as if
/// it were connected). A preview never writes to the device or to the settings file.
fn apply_preview(ui: &AppWindow, app: &Rc<App>) {
    let Ok(preview) = std::env::var("AM8_LITE_PREVIEW") else { return };
    match preview.as_str() {
        "close" => ui.set_close_prompt(true),
        "reset" => ui.set_confirm_reset(true),
        "collapsed" => ui.set_sidebar_collapsed(true),
        "effects" => {
            ui.set_page(2);
            ui.set_light_mode(0);
        }
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
            } else if let Some(model) = other.strip_prefix("model:").and_then(profile::by_id) {
                app.frozen.set(true);
                app.show_profile(ui, model);
                ui.set_device_ready(true);
                ui.set_light_on(true);
            }
        }
    }
    app.update_ring(ui);
}

/// Lighting page: effect names, built-in effects, custom colours and animations.
fn wire_lighting(ui: &AppWindow, app: &Rc<App>) {
    let state = app.clone();
    ui.on_effect_name_edited(move |index, text| {
        let Ok(index) = usize::try_from(index) else { return };
        let profile = state.profile.get();
        if index >= usize::from(profile.patterns) {
            return;
        }
        {
            let mut settings = state.settings.borrow_mut();
            let names = &mut settings.light_mut(profile.id).effect_names;
            names.resize(usize::from(profile.patterns), String::new());
            names[index] = text.trim().to_owned();
        }
        state.save_settings();
        let Some(ui) = state.ui.upgrade() else { return };
        // Update only the edited entry so the text field keeps its cursor.
        let model = ui.get_effect_names();
        if let Some(names) = model.as_any().downcast_ref::<slint::VecModel<SharedString>>() {
            let label = if text.trim().is_empty() {
                format!("{} {}", tr("Effect", "Эффект"), index + 1)
            } else {
                text.trim().to_owned()
            };
            names.set_row_data(index, label.into());
        }
    });

    // Slider drags fire many changes; save settings once things settle.
    let save_timer = slint::Timer::default();
    let previous_mode = Cell::new(ui.get_light_mode());
    let state = app.clone();
    ui.on_light_config_changed(move || {
        let Some(ui) = state.ui.upgrade() else { return };
        let profile = state.profile.get();
        {
            let mut settings = state.settings.borrow_mut();
            let light = settings.light_mut(profile.id);
            light.light_mode = ui.get_light_mode().clamp(0, 2) as u8;
            light.per_led = ui.get_per_led();
            light.animation = ui.get_anim_index().clamp(0, 3) as u8;
            light.anim_speed = ui.get_anim_speed();
            light.second_hue = ui.get_second_hue();
            light.custom_color = [ui.get_custom_hue(), ui.get_custom_sat(), ui.get_custom_value()];
            light.leds = state.leds.borrow().clone();
        }
        let saver = state.clone();
        save_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(400), move || saver.save_settings());
        state.update_ring(&ui);

        let was = previous_mode.replace(ui.get_light_mode());
        let usable = ui.get_light_on() && ui.get_device_ready() && profile.leds > 0;
        match state.light_mode(&ui) {
            None => {
                state.send(WriteCommand::Light(None));
                if was != 0 && usable && profile.patterns > 0 {
                    // Hand the ring back to the selected built-in effect.
                    ui.invoke_setting_changed("light-pattern".into(), ui.get_light_pattern_index() + 1);
                }
            }
            Some(mode) if usable => state.send(WriteCommand::Light(Some(mode))),
            Some(_) => state.send(WriteCommand::Light(None)),
        }
    });

    let state = app.clone();
    ui.on_led_painted(move |index| {
        let Some(ui) = state.ui.upgrade() else { return };
        let hsv = [ui.get_custom_hue(), ui.get_custom_sat(), ui.get_custom_value()];
        let Ok(index) = usize::try_from(index) else { return };
        match state.leds.borrow_mut().get_mut(index) {
            Some(slot) => *slot = hsv,
            None => return,
        }
        ui.invoke_light_config_changed();
    });

    let state = app.clone();
    ui.on_led_fill_all(move || {
        let Some(ui) = state.ui.upgrade() else { return };
        let hsv = [ui.get_custom_hue(), ui.get_custom_sat(), ui.get_custom_value()];
        state.leds.borrow_mut().fill(hsv);
        ui.invoke_light_config_changed();
    });
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
