//! Read-only diagnostic: lists the supported microphones that are plugged in and prints
//! what the first one reports.
use fifine_lite::{
    device::{DeviceEvent, WriteCommand, spawn_device},
    level::LevelMonitor,
};
use std::{sync::mpsc, time::Duration};

fn main() {
    let (events, received) = mpsc::channel();
    let (commands, thread) = spawn_device(LevelMonitor::spawn(), move |event| {
        let _ = events.send(event);
    });
    let mut reports = 0;
    while let Ok(event) = received.recv_timeout(Duration::from_secs(5)) {
        match event {
            DeviceEvent::Devices(devices) => {
                for device in devices {
                    println!("found {} ({})", device.profile.name, device.key);
                }
            }
            DeviceEvent::Connected(device) => println!("connected to {}", device.profile.name),
            DeviceEvent::State(state) => {
                println!("{state:?}");
                reports += 1;
                if reports == 2 {
                    break;
                }
            }
            DeviceEvent::Disconnected(problem) | DeviceEvent::Rejected(problem) => eprintln!("{problem:?}"),
        }
    }
    let _ = commands.send(WriteCommand::Quit(None));
    let _ = thread.join();
}
