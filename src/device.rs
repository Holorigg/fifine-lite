//! The one thread that talks to the microphones over HID: it finds the supported ones,
//! applies settings to the selected one, streams its lighting and polls its state.
use crate::level::{Consumer, LevelMonitor};
use crate::light::{Frame, LightMode, clock};
use crate::profile::{self, Profile, USAGE, USAGE_PAGE, VENDOR_ID};
use crate::protocol::{
    DeviceState, EqChannel, EqPoint, PATTERN_CUSTOM, QUERY_REPORT, Setting, eq_report, frame_report,
    parse_input_report, reset_report, setting_report,
};
use hidapi::{HidApi, HidDevice};
use std::ffi::CString;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// A supported microphone that is plugged in.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    /// Identifies this very unit while it stays plugged in (two of the same model differ).
    pub key: String,
    pub profile: &'static Profile,
}

pub enum WriteCommand {
    /// Control this microphone from now on.
    Select(String),
    Setting(Setting, u8),
    Eq(EqChannel, Vec<EqPoint>),
    /// Custom lighting. `Some` shows the mode (frames keep streaming where the microphone
    /// needs them), `None` stops sending frames.
    Light(Option<LightMode>),
    Reset,
    /// Optionally leaves a final frame on the ring, then ends the thread.
    Quit(Option<Frame>),
}

/// Why something did not work; the app words it in the user's language.
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    NotFound,
    NotResponding,
    CannotOpen(String),
    InvalidValue,
    InvalidEq,
}

pub enum DeviceEvent {
    /// The set of plugged-in supported microphones changed.
    Devices(Vec<DeviceInfo>),
    /// This microphone is now the one being controlled.
    Connected(DeviceInfo),
    State(DeviceState),
    /// The controlled microphone is unplugged or stopped answering.
    Disconnected(Problem),
    /// A command was refused; the connection itself is fine.
    Rejected(Problem),
}

/// How often the state is read back.
const POLL: Duration = Duration::from_millis(750);
/// Interval between frames of an animation (Genie streams at 30 fps).
const FRAME: Duration = Duration::from_millis(33);
/// Interval between repeats of a still frame for microphones that drop custom colours
/// when frames stop arriving.
const KEEP_ALIVE: Duration = Duration::from_millis(150);
/// How often the list of plugged-in microphones is refreshed.
const RESCAN: Duration = Duration::from_millis(1500);
const RESCAN_CONNECTED: Duration = Duration::from_secs(3);
/// Unreadable state reports in a row before the connection is reopened.
const MAX_BAD_REPORTS: u32 = 5;

enum Failure {
    Rejected(Problem),
    /// The microphone is gone or not answering.
    Io,
}

struct Found {
    info: DeviceInfo,
    path: CString,
}

struct Open {
    info: DeviceInfo,
    hid: HidDevice,
}

/// Lists the control interface of every supported microphone.
fn scan(api: &mut Option<HidApi>) -> Vec<Found> {
    let api = match api {
        Some(api) => {
            if api.refresh_devices().is_err() {
                return Vec::new();
            }
            api
        }
        None => match HidApi::new() {
            Ok(created) => api.insert(created),
            Err(_) => return Vec::new(),
        },
    };
    // (unit, uses the usual vendor collection)
    let mut found: Vec<(Found, String, bool)> = Vec::new();
    for device in api.device_list().filter(|device| device.vendor_id() == VENDOR_ID) {
        let Some(profile) = profile::by_product_id(device.product_id()) else { continue };
        let exact = device.usage_page() == USAGE_PAGE && device.usage() == USAGE;
        // Without the usual collection, any vendor-defined one of that microphone will do.
        if !exact && device.usage_page() < 0xff00 {
            continue;
        }
        let unit = device.serial_number().unwrap_or_default().to_owned();
        let path = device.path().to_owned();
        let key = format!("{}|{}", profile.id, path.to_string_lossy());
        found.push((Found { info: DeviceInfo { key, profile }, path }, unit, exact));
    }
    // One microphone exposes several interfaces: keep the usual collection when the model
    // has it, otherwise one fallback interface per unit.
    let mut kept: Vec<(Found, String, bool)> = Vec::new();
    for entry in found {
        let model = entry.0.info.profile.id;
        let model_has_exact = entry.2 || kept.iter().any(|other| other.0.info.profile.id == model && other.2);
        if entry.2 {
            kept.retain(|other| other.0.info.profile.id != model || other.2);
            kept.push(entry);
        } else if !model_has_exact && !kept.iter().any(|other| other.0.info.profile.id == model && other.1 == entry.1) {
            kept.push(entry);
        }
    }
    kept.into_iter().map(|entry| entry.0).collect()
}

fn write(device: &HidDevice, bytes: &[u8]) -> Result<(), Failure> {
    device.write(bytes).map(|_| ()).map_err(|_| Failure::Io)
}

fn write_setting(open: &Open, setting: Setting, value: u8) -> Result<(), Failure> {
    let profile = open.info.profile;
    let codes = &profile.codes;
    let invalid = || Failure::Rejected(Problem::InvalidValue);
    let code = setting.code(codes).ok_or_else(invalid)?;
    let allowed = match setting {
        Setting::InputVolume | Setting::HeadphoneVolume => value <= profile.volume_max,
        Setting::MonitorMix => value <= profile.mix_max,
        Setting::LightPattern => {
            (1..=profile.patterns).contains(&value) || (profile.holds_custom_light && value == PATTERN_CUSTOM)
        }
        Setting::Voice => value == 0 || (2..=8).contains(&value),
        Setting::Noise => value <= profile.noise_levels,
        Setting::Pickup => (1..=4).contains(&value),
        Setting::LightSwitch | Setting::Mute | Setting::LowCut | Setting::MidBoost => value <= 1,
    };
    if !allowed {
        return Err(invalid());
    }
    let volume_mode = match setting {
        Setting::InputVolume => Some(0),
        Setting::HeadphoneVolume => Some(1),
        Setting::MonitorMix => Some(2),
        _ => None,
    };
    if let (Some(mode), Some(mode_code)) = (volume_mode, codes.volume_mode) {
        write(&open.hid, &setting_report(mode_code, mode))?;
    }
    write(&open.hid, &setting_report(code, value))?;
    if let (Setting::Voice, Some(realtime)) = (setting, codes.realtime) {
        thread::sleep(Duration::from_millis(100));
        write(&open.hid, &setting_report(realtime, 1))?;
    }
    Ok(())
}

/// `Ok(None)` is a report that could not be understood; the next poll usually succeeds.
fn read_state(open: &Open) -> Result<Option<DeviceState>, Failure> {
    let profile = open.info.profile;
    let Some(layout) = &profile.layout else {
        // Nothing to read; an empty write still shows whether the microphone is there.
        write(&open.hid, &QUERY_REPORT)?;
        return Ok(Some(DeviceState::default()));
    };
    write(&open.hid, &QUERY_REPORT)?;
    let mut buffer = [0u8; 257];
    let length = open.hid.get_input_report(&mut buffer).map_err(|_| Failure::Io)?;
    Ok(parse_input_report(&buffer[1..length.max(1)], layout, profile.has_eq).ok())
}

pub fn spawn_device(
    level: LevelMonitor,
    emit: impl Fn(DeviceEvent) + Send + 'static,
) -> (Sender<WriteCommand>, JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel::<WriteCommand>();
    let handle = thread::spawn(move || {
        let mut api: Option<HidApi> = None;
        let mut present: Vec<Found> = Vec::new();
        let mut open: Option<Open> = None;
        // The microphone the user picked; the first one found is used otherwise.
        let mut wanted: Option<String> = None;
        let mut light: Option<LightMode> = None;
        // True once this connection has been told to keep showing our frames.
        let mut custom_held = false;
        let mut bad_reports = 0;
        let mut missing_reported = false;
        let mut next_poll = Instant::now();
        let mut next_frame = Instant::now();
        let mut next_scan = Instant::now();
        loop {
            // Still frames are repeated for microphones that would otherwise drop them.
            let frame_interval = match (&open, light) {
                (Some(_), Some(mode)) if mode.animated() => Some(FRAME),
                (Some(open), Some(_)) if !open.info.profile.holds_custom_light => Some(KEEP_ALIVE),
                _ => None,
            };
            let mut wake = next_scan;
            if open.is_some() {
                wake = wake.min(next_poll);
            }
            if frame_interval.is_some() {
                wake = wake.min(next_frame);
            }
            let command = match receiver.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            match &command {
                Some(WriteCommand::Light(mode)) => {
                    light = *mode;
                    level.set_wanted(Consumer::Light, light.is_some_and(|mode| mode.needs_level()));
                    if light.is_none() {
                        custom_held = false;
                    }
                }
                Some(WriteCommand::Select(key)) => {
                    wanted = Some(key.clone());
                    if open.as_ref().is_some_and(|open| &open.info.key != key) {
                        open = None;
                        light = None;
                        level.set_wanted(Consumer::Light, false);
                    }
                    next_scan = Instant::now();
                }
                _ => {}
            }

            if Instant::now() >= next_scan || (open.is_none() && command.is_some()) {
                next_scan = Instant::now() + if open.is_some() { RESCAN_CONNECTED } else { RESCAN };
                let found = scan(&mut api);
                let changed = found.len() != present.len()
                    || found.iter().zip(&present).any(|(new, old)| new.info.key != old.info.key);
                present = found;
                if changed {
                    emit(DeviceEvent::Devices(present.iter().map(|found| found.info.clone()).collect()));
                }
                if open.as_ref().is_some_and(|open| !present.iter().any(|found| found.info.key == open.info.key)) {
                    open = None;
                    missing_reported = true;
                    emit(DeviceEvent::Disconnected(Problem::NotFound));
                }
            }

            if open.is_none() {
                let target = wanted
                    .as_ref()
                    .and_then(|key| present.iter().find(|found| &found.info.key == key))
                    .or(present.first());
                match (target, api.as_ref()) {
                    (Some(found), Some(api)) => match api.open_path(&found.path) {
                        Ok(hid) => {
                            emit(DeviceEvent::Connected(found.info.clone()));
                            open = Some(Open { info: found.info.clone(), hid });
                            custom_held = false;
                            bad_reports = 0;
                            missing_reported = false;
                            next_poll = Instant::now();
                        }
                        Err(error) => {
                            if !missing_reported {
                                missing_reported = true;
                                emit(DeviceEvent::Disconnected(Problem::CannotOpen(error.to_string())));
                            }
                        }
                    },
                    _ => {
                        if !missing_reported {
                            missing_reported = true;
                            emit(DeviceEvent::Disconnected(Problem::NotFound));
                        }
                    }
                }
            }
            let Some(device) = &open else {
                if matches!(command, Some(WriteCommand::Quit(_))) {
                    break;
                }
                continue;
            };
            let profile = device.info.profile;

            let mut show_frame = |frame: &Frame| -> Result<(), Failure> {
                if profile.leds == 0 {
                    return Ok(());
                }
                write(&device.hid, &frame_report(frame, profile.leds))?;
                if let (true, false, Some(code)) = (profile.holds_custom_light, custom_held, profile.codes.light_pattern) {
                    // Without this the ring falls back to its built-in effect half a second
                    // after the last frame.
                    write(&device.hid, &setting_report(code, PATTERN_CUSTOM))?;
                    custom_held = true;
                }
                Ok(())
            };

            let mut result = match command {
                Some(WriteCommand::Quit(frame)) => {
                    if let Some(frame) = frame {
                        let _ = show_frame(&frame);
                    }
                    break;
                }
                Some(WriteCommand::Light(_)) => match light {
                    Some(mode) => {
                        next_frame = Instant::now() + frame_interval.unwrap_or(FRAME);
                        show_frame(&mode.frame(clock(), level.peak(), profile.leds))
                    }
                    None => Ok(()),
                },
                Some(WriteCommand::Setting(setting, value)) => write_setting(device, setting, value),
                Some(WriteCommand::Eq(channel, points)) => match eq_report(channel, &points) {
                    Ok(bytes) if profile.has_eq => write(&device.hid, &bytes),
                    _ => Err(Failure::Rejected(Problem::InvalidEq)),
                },
                Some(WriteCommand::Reset) if profile.has_reset => write(&device.hid, &reset_report()),
                Some(WriteCommand::Reset | WriteCommand::Select(_)) | None => Ok(()),
            };
            if let (Ok(()), Some(interval), Some(mode)) = (&result, frame_interval, light) {
                if Instant::now() >= next_frame {
                    next_frame = Instant::now() + interval;
                    result = show_frame(&mode.frame(clock(), level.peak(), profile.leds));
                }
            }
            if result.is_ok() && Instant::now() >= next_poll {
                next_poll = Instant::now() + POLL;
                result = match read_state(device) {
                    Ok(Some(state)) => {
                        bad_reports = 0;
                        emit(DeviceEvent::State(state));
                        Ok(())
                    }
                    Ok(None) => {
                        bad_reports += 1;
                        if bad_reports >= MAX_BAD_REPORTS { Err(Failure::Io) } else { Ok(()) }
                    }
                    Err(failure) => Err(failure),
                };
            }
            match result {
                Ok(()) => {}
                Err(Failure::Rejected(problem)) => emit(DeviceEvent::Rejected(problem)),
                Err(Failure::Io) => {
                    open = None;
                    custom_held = false;
                    missing_reported = true;
                    next_scan = Instant::now() + RESCAN;
                    emit(DeviceEvent::Disconnected(Problem::NotResponding));
                }
            }
        }
    });
    (sender, handle)
}
