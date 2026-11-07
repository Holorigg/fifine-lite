//! Microphone input level, captured from the AM8 Pro's Windows audio endpoint.
//!
//! Capture runs only while someone asks for it (the visible level meter or the
//! voice-reactive light), because Windows shows "microphone in use" while it is open.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::thread;
use std::time::Duration;

/// Who currently needs the level. Capture is open while any flag is set.
#[derive(Clone, Copy)]
pub enum Consumer {
    Meter = 0,
    Light = 1,
}

#[derive(Clone)]
pub struct LevelMonitor {
    /// Peak level 0–1 (linear), stored as f32 bits; decays smoothly between buffers.
    peak: Arc<AtomicU32>,
    wanted: Arc<[AtomicBool; 2]>,
    active: Arc<AtomicBool>,
}

impl LevelMonitor {
    pub fn spawn() -> Self {
        let monitor = Self {
            peak: Arc::new(AtomicU32::new(0f32.to_bits())),
            wanted: Arc::new([AtomicBool::new(false), AtomicBool::new(false)]),
            active: Arc::new(AtomicBool::new(false)),
        };
        let worker = monitor.clone();
        thread::Builder::new()
            .name("level-monitor".into())
            .spawn(move || worker.run())
            .expect("spawn level monitor");
        monitor
    }

    pub fn set_wanted(&self, consumer: Consumer, wanted: bool) {
        self.wanted[consumer as usize].store(wanted, Ordering::Relaxed);
    }

    /// Linear peak 0–1.
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    /// Peak in dBFS, floored at -60.
    pub fn peak_db(&self) -> f32 {
        let peak = self.peak();
        if peak <= 0.001 { -60.0 } else { (20.0 * peak.log10()).max(-60.0) }
    }

    /// True while the capture stream is open.
    pub fn capturing(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    fn any_wanted(&self) -> bool {
        self.wanted.iter().any(|w| w.load(Ordering::Relaxed))
    }

    fn run(self) {
        loop {
            if !self.any_wanted() {
                thread::sleep(Duration::from_millis(200));
                continue;
            }
            match self.open_stream() {
                Ok(stream) => {
                    self.active.store(true, Ordering::Relaxed);
                    while self.any_wanted() {
                        thread::sleep(Duration::from_millis(100));
                    }
                    drop(stream);
                    self.active.store(false, Ordering::Relaxed);
                    self.peak.store(0f32.to_bits(), Ordering::Relaxed);
                }
                // Microphone missing or busy: try again a bit later.
                Err(_) => thread::sleep(Duration::from_secs(2)),
            }
        }
    }

    fn open_stream(&self) -> Result<cpal::Stream, String> {
        let host = cpal::default_host();
        let device = host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|device| device.to_string().contains("AM8"))
            .or_else(|| host.default_input_device())
            .ok_or("нет устройства ввода")?;
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        let peak = self.peak.clone();
        // Per-buffer peak with a smooth fall-off so the meter does not flicker.
        let mut held = 0f32;
        let mut on_samples = move |samples: &mut dyn Iterator<Item = f32>| {
            let buffer_peak = samples.fold(0f32, |acc, s| acc.max(s.abs())).min(1.0);
            held = if buffer_peak >= held { buffer_peak } else { held * 0.85 + buffer_peak * 0.15 };
            peak.store(held.to_bits(), Ordering::Relaxed);
        };
        let on_error = |_| {};
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                config,
                move |data: &[f32], _: &_| on_samples(&mut data.iter().copied()),
                on_error,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                config,
                move |data: &[i16], _: &_| on_samples(&mut data.iter().map(|&s| f32::from(s) / 32768.0)),
                on_error,
                None,
            ),
            cpal::SampleFormat::I32 => device.build_input_stream(
                config,
                move |data: &[i32], _: &_| on_samples(&mut data.iter().map(|&s| s as f32 / 2_147_483_648.0)),
                on_error,
                None,
            ),
            other => return Err(format!("формат {other:?} не поддержан")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(stream)
    }
}
