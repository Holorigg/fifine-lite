//! Diagnostic: opens the AM8 Pro capture for a few seconds and prints the peak level.
use fifine_am8_lite::level::{Consumer, LevelMonitor};
use std::{thread, time::Duration};

fn main() {
    let level = LevelMonitor::spawn();
    level.set_wanted(Consumer::Meter, true);
    for _ in 0..12 {
        thread::sleep(Duration::from_millis(250));
        println!("capturing={} peak={:.1} dBFS", level.capturing(), level.peak_db());
    }
    level.set_wanted(Consumer::Meter, false);
    thread::sleep(Duration::from_millis(300));
    println!("after release: capturing={}", level.capturing());
}
