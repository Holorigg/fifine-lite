use fifine_am8_lite::{
    device::{WriteCommand, spawn_writer},
    helper::{ReaderEvent, spawn_reader},
    protocol::{EqChannel, EqPoint, PRODUCT_ID, Setting, USAGE, USAGE_PAGE, VENDOR_ID, eq_report},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api = hidapi::HidApi::new()?;
    let info = api
        .device_list()
        .find(|info| {
            info.vendor_id() == VENDOR_ID
                && info.product_id() == PRODUCT_ID
                && info.usage() == USAGE
                && info.usage_page() == USAGE_PAGE
        })
        .ok_or("AM8 Pro control interface not found")?;
    let _device = info.open_device(&api)?;
    let (sender, receiver) = mpsc::channel();
    let running = Arc::new(AtomicBool::new(true));
    let reader = spawn_reader(running.clone(), move |event| {
        let _ = sender.send(event);
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut channels = [false; 2];
    let mut configuration = None;
    let mut microphone_eq = None;
    while let Ok(event) = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        match event {
            ReaderEvent::State(state) => {
                channels[usize::from(state.channel == EqChannel::Microphone)] = true;
                if let Err(error) = eq_report(state.channel, &state.eq_points) {
                    eprintln!("Existing EQ cannot be encoded: {error}");
                }
                if state.channel == EqChannel::Microphone && state.configuration.mic_mute {
                    microphone_eq = Some(state.eq_points.clone());
                }
                println!(
                    "{:?}: {} EQ points, {:?}",
                    state.channel,
                    state.eq_points.len(),
                    state.configuration
                );
                configuration = Some(state.configuration);
                if channels == [true, true] {
                    break;
                }
            }
            ReaderEvent::Disconnected(error) => eprintln!("{error}"),
            ReaderEvent::Connected => {}
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    let verification = if std::env::args().any(|arg| arg == "--verify-write") {
        verify_light_switch(configuration, &receiver)
            .and_then(|()| verify_microphone_eq(microphone_eq, &receiver))
    } else {
        Ok(())
    };
    running.store(false, Ordering::Relaxed);
    let _ = reader.join();
    if channels != [true, true] {
        return Err("Did not receive both EQ channels".into());
    }
    verification?;
    Ok(())
}

fn verify_microphone_eq(
    original: Option<Vec<EqPoint>>,
    receiver: &mpsc::Receiver<ReaderEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let original = original.ok_or("Microphone must be muted for the EQ write check")?;
    let mut temporary = original.clone();
    let first = temporary
        .first_mut()
        .ok_or("No active microphone EQ points")?;
    first.gain_db += if first.gain_db > 11.75 { -0.25 } else { 0.25 };
    let writer = spawn_writer(fifine_am8_lite::level::LevelMonitor::spawn(), |error| eprintln!("{error}"));
    let write =
        |points: &[EqPoint]| writer.send(WriteCommand::Eq(EqChannel::Microphone, points.to_vec()));
    let observe = |points: &[EqPoint]| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let Ok(event) =
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            else {
                break;
            };
            if let ReaderEvent::State(state) = event {
                if state.channel == EqChannel::Microphone && state.eq_points == points {
                    return true;
                }
            }
        }
        false
    };
    write(&temporary)?;
    let changed = observe(&temporary);
    write(&original)?;
    if !observe(&original) {
        return Err("Could not verify restoration of the original microphone EQ".into());
    }
    if !changed {
        return Err("Microphone EQ change was not observed; original confirmed".into());
    }
    println!("Verified microphone EQ write and restoration of all points");
    Ok(())
}

fn verify_light_switch(
    configuration: Option<fifine_am8_lite::protocol::Configuration>,
    receiver: &mpsc::Receiver<ReaderEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let configuration = configuration.ok_or("No configuration received")?;
    let original = configuration.light_on;
    let writer = spawn_writer(fifine_am8_lite::level::LevelMonitor::spawn(), |error| eprintln!("{error}"));
    let write = |enabled| {
        writer.send(WriteCommand::Setting(
            Setting::LightSwitch,
            u8::from(enabled),
        ))
    };
    let observe = |enabled| {
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            let Ok(event) =
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            else {
                break;
            };
            if let ReaderEvent::State(state) = event {
                if state.configuration.light_on == enabled {
                    return true;
                }
            }
        }
        false
    };
    write(!original)?;
    let changed = observe(!original);
    write(original)?;
    if !observe(original) {
        return Err(format!("Could not verify restoration of light switch to {original}").into());
    }
    if !changed {
        return Err("Light switch change was not observed; original confirmed".into());
    }
    println!(
        "Verified HID write and restoration: light {original} → {} → {original}",
        !original
    );
    Ok(())
}
