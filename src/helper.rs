use crate::protocol::{DeviceState, parse_helper_hex_report};
use serde_json::{Value, json};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket, connect, stream::MaybeTlsStream};

const URL: &str = "ws://127.0.0.1:24925";
const HELPER_PATH: &str = "C:\\Program Files\\fifine Genie\\fifine Genie\\resources\\app\\dist\\AudioDriverControlerRelease\\fifineAudioDriverControler.exe";

pub enum ReaderEvent {
    Connected,
    Disconnected(String),
    State(DeviceState),
}

fn send(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    event: &str,
    data: Option<&[u8]>,
) -> tungstenite::Result<()> {
    let device = json!({
        "PID": 424, "VID": 12610, "usage": 0x55aa, "usagePage": 0xff00, "reportID": 0
    });
    let payload = if let Some(bytes) = data {
        json!({"device": device, "data": bytes})
    } else {
        json!({"device": device})
    };
    socket.send(Message::Text(
        json!({"event": event, "payload": payload})
            .to_string()
            .into(),
    ))
}

fn parse_message(text: &str) -> Option<DeviceState> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("event")?.as_str()? != "receivedFromGetInputReport" {
        return None;
    }
    let bytes = value.get("payload")?.get("data")?.as_array()?;
    let hex = bytes
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    parse_helper_hex_report(&hex).ok()
}

fn start_helper() -> std::io::Result<Child> {
    let mut command = Command::new(HELPER_PATH);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command.spawn()
}

/// Keeps Genie's small controller alive only while this app needs it.
/// The reader sends the same read request as Genie; it never changes a setting.
pub fn spawn_reader(
    running: Arc<AtomicBool>,
    emit: impl Fn(ReaderEvent) + Send + 'static,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut owned_helper: Option<Child> = None;
        let mut failures = 0;
        while running.load(Ordering::Relaxed) {
            let (mut socket, _) = match connect(URL) {
                Ok(connection) => connection,
                Err(error) => {
                    failures += 1;
                    if failures == 3 && owned_helper.is_none() {
                        match start_helper() {
                            Ok(child) => owned_helper = Some(child),
                            Err(start_error) => emit(ReaderEvent::Disconnected(format!(
                                "Не удалось запустить helper: {start_error}"
                            ))),
                        }
                    }
                    if failures % 5 == 1 {
                        emit(ReaderEvent::Disconnected(format!(
                            "Нет связи с helper: {error}"
                        )));
                    }
                    thread::sleep(Duration::from_millis(800));
                    continue;
                }
            };
            failures = 0;
            if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(150)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            }
            if send(&mut socket, "openHidDevice", None).is_err() {
                continue;
            }
            emit(ReaderEvent::Connected);
            let mut last_query = Instant::now() - Duration::from_secs(1);
            let mut last_report = Instant::now();
            while running.load(Ordering::Relaxed) {
                if last_report.elapsed() >= Duration::from_secs(5) {
                    break;
                }
                if last_query.elapsed() >= Duration::from_millis(750) {
                    if send(
                        &mut socket,
                        "writeDataToHidDevice",
                        Some(&[0xa5, 0x5a, 0xfe, 0, 0x16]),
                    )
                    .is_err()
                        || send(&mut socket, "getInputReportFromHidDevice", None).is_err()
                    {
                        break;
                    }
                    last_query = Instant::now();
                }
                match socket.read() {
                    Ok(Message::Text(text)) => {
                        if let Some(state) = parse_message(&text) {
                            last_report = Instant::now();
                            emit(ReaderEvent::State(state));
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(tungstenite::Error::Io(error))
                        if error.kind() == std::io::ErrorKind::TimedOut
                            || error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                    _ => {}
                }
            }
            let _ = socket.close(None);
            if running.load(Ordering::Relaxed) {
                emit(ReaderEvent::Disconnected(
                    "Нет ответа от AM8 Pro. Повторное подключение…".to_owned(),
                ));
                thread::sleep(Duration::from_millis(400));
            }
        }
        if let Some(mut child) = owned_helper {
            let _ = child.kill();
            let _ = child.wait();
        }
    })
}
