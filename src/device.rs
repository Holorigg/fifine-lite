use crate::level::{Consumer, LevelMonitor};
use crate::light::LightMode;
use crate::protocol::{
    EqChannel, EqPoint, PRODUCT_ID, Setting, USAGE, USAGE_PAGE, VENDOR_ID, eq_report,
    frame_report, setting_report,
};
use hidapi::{HidApi, HidDevice};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

pub enum WriteCommand {
    Setting(Setting, u8),
    Eq(EqChannel, Vec<EqPoint>),
    /// Custom lighting. The AM8 Pro falls back to its built-in effect unless colour frames
    /// keep arriving (Genie streams them at 30 fps), so `Some` starts a continuous stream of
    /// frames from the writer thread and `None` stops it.
    Light(Option<LightMode>),
    Reset,
}

fn open_am8_pro() -> Result<HidDevice, String> {
    let api = HidApi::new().map_err(|e| e.to_string())?;
    let info = api
        .device_list()
        .find(|device| {
            device.vendor_id() == VENDOR_ID
                && device.product_id() == PRODUCT_ID
                && device.usage_page() == USAGE_PAGE
                && device.usage() == USAGE
        })
        .ok_or_else(|| "AM8 Pro не найден в HID".to_owned())?;
    info.open_device(&api).map_err(|e| e.to_string())
}

fn write(device: &HidDevice, bytes: &[u8]) -> Result<(), String> {
    device.write(bytes).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_command(device: &HidDevice, command: WriteCommand) -> Result<(), String> {
    match command {
        WriteCommand::Setting(setting, value) => {
            match setting {
                Setting::InputVolume if value <= 27 => {
                    write(device, &setting_report(Setting::VolumeMode, 0))?
                }
                Setting::HeadphoneVolume if value <= 27 => {
                    write(device, &setting_report(Setting::VolumeMode, 1))?
                }
                Setting::MonitorMix if value <= 24 => {
                    write(device, &setting_report(Setting::VolumeMode, 2))?
                }
                Setting::LightPattern if (1..=15).contains(&value) => {}
                Setting::VoiceChanger if value == 0 || (2..=8).contains(&value) => {}
                Setting::LightSwitch | Setting::NoiseSwitch | Setting::MicMute if value <= 1 => {}
                _ => return Err("Недопустимое значение настройки AM8 Pro".to_owned()),
            }
            write(device, &setting_report(setting, value))?;
            if matches!(setting, Setting::VoiceChanger) {
                thread::sleep(Duration::from_millis(100));
                write(device, &setting_report(Setting::RealTimeSwitch, 1))?;
            }
        }
        WriteCommand::Eq(channel, points) => {
            let bytes = eq_report(channel, &points).map_err(str::to_owned)?;
            write(device, &bytes)?;
        }
        // Frames are written by the stream loop in `spawn_writer`.
        WriteCommand::Light(_) => {}
        WriteCommand::Reset => write(device, &setting_report(Setting::Reset, 0))?,
    }
    Ok(())
}

/// Interval between colour frames while a custom colour is active.
const COLOR_FRAME: Duration = Duration::from_millis(40);
/// Retry interval while streaming but the microphone cannot be opened.
const REOPEN_DELAY: Duration = Duration::from_secs(1);

pub fn spawn_writer(
    level: LevelMonitor,
    report_error: impl Fn(String) + Send + 'static,
) -> Sender<WriteCommand> {
    let (sender, receiver): (Sender<WriteCommand>, Receiver<WriteCommand>) = mpsc::channel();
    thread::spawn(move || {
        let mut device: Option<HidDevice> = None;
        let mut stream: Option<LightMode> = None;
        let started = Instant::now();
        loop {
            // Block indefinitely when idle; wake up for the next frame while streaming.
            let command = match stream {
                None => match receiver.recv() {
                    Ok(command) => Some(command),
                    Err(_) => break,
                },
                Some(_) => {
                    let wait = if device.is_some() { COLOR_FRAME } else { REOPEN_DELAY };
                    match receiver.recv_timeout(wait) {
                        Ok(command) => Some(command),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            };
            if let Some(WriteCommand::Light(mode)) = &command {
                stream = *mode;
                level.set_wanted(Consumer::Light, stream.is_some_and(|m| m.needs_level()));
            }
            // A stream tick that cannot reach the device just retries later, silently.
            let user_command = command.is_some();
            if device.is_none() {
                match open_am8_pro() {
                    Ok(opened) => device = Some(opened),
                    Err(error) => {
                        if user_command {
                            report_error(error);
                        }
                        continue;
                    }
                }
            }
            let result = match (command, stream) {
                // A new or repeated light mode, or a stream tick: write the next frame.
                (Some(WriteCommand::Light(_)) | None, Some(mode)) => {
                    let frame = mode.frame(started.elapsed().as_secs_f32(), level.peak());
                    write(device.as_ref().unwrap(), &frame_report(&frame))
                }
                (Some(command), _) => write_command(device.as_ref().unwrap(), command),
                (None, None) => continue,
            };
            if let Err(error) = result {
                if user_command {
                    report_error(format!("Ошибка записи AM8 Pro: {error}"));
                }
                device = None;
            }
        }
    });
    sender
}
