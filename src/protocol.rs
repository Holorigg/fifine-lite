//! Wire format shared by fifine's `A5 5A FE` microphones. Which codes and report offsets a
//! given model uses is described by its [`Profile`](crate::profile::Profile).
use crate::profile::{Codes, Field, Layout, MAX_LEDS};
use serde::{Deserialize, Serialize};

/// A setting the app can change. Models support different subsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    LightSwitch,
    LightPattern,
    Mute,
    Noise,
    Voice,
    InputVolume,
    HeadphoneVolume,
    MonitorMix,
    Pickup,
    LowCut,
    MidBoost,
}

impl Setting {
    /// Command code of this setting on a given model.
    pub fn code(self, codes: &Codes) -> Option<u8> {
        match self {
            Setting::LightSwitch => codes.light_switch,
            Setting::LightPattern => codes.light_pattern,
            Setting::Mute => codes.mute,
            Setting::Noise => codes.noise,
            Setting::Voice => codes.voice,
            Setting::InputVolume => codes.input_volume,
            Setting::HeadphoneVolume => codes.headphone_volume,
            Setting::MonitorMix => codes.monitor_mix,
            Setting::Pickup => codes.pickup,
            Setting::LowCut => codes.low_cut,
            Setting::MidBoost => codes.mid_boost,
        }
    }
}

/// Report ID precedes the payload, matching Genie's configData().
pub fn setting_report(code: u8, value: u8) -> Vec<u8> {
    vec![0, 0xa5, 0x5a, 0xfe, 0x02, code, value, 0x16]
}

/// Factory reset.
pub fn reset_report() -> Vec<u8> {
    vec![0, 0xa5, 0x5a, 0xfe, 0x01, 0x20, 0x16]
}

/// Asks the microphone to refresh its state report (report ID first).
pub const QUERY_REPORT: [u8; 6] = [0, 0xa5, 0x5a, 0xfe, 0x00, 0x16];

/// Light pattern the AM8 Pro reports while it shows colours sent by the computer.
/// Lower numbers are built-in effects. Selecting 16 makes the last frame stick.
pub const PATTERN_CUSTOM: u8 = 16;

/// One colour per LED, matching Genie's Render(): A5 5A FE len 0B n, n×RGB, 16.
/// Sent with report ID 0 and padded to the 256-byte transfer like the EQ report.
pub fn frame_report(frame: &[[u8; 3]; MAX_LEDS], leds: usize) -> Vec<u8> {
    let leds = leds.min(MAX_LEDS);
    let mut bytes = vec![0, 0xa5, 0x5a, 0xfe, (2 + 3 * leds) as u8, 0x0b, leds as u8];
    for rgb in &frame[..leds] {
        bytes.extend_from_slice(rgb);
    }
    bytes.push(0x16);
    bytes.resize(257, 0);
    bytes
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EqPoint {
    pub filter_type: u16,
    pub frequency_hz: u16,
    pub q: f32,
    pub gain_db: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum EqChannel {
    Output,
    Microphone,
}

/// What a state report says. A value is `None` when the model has no such setting.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeviceState {
    pub light_on: Option<bool>,
    pub light_pattern: Option<u8>,
    pub mic_mute: Option<bool>,
    pub noise: Option<u8>,
    pub voice: Option<u8>,
    pub input_volume: Option<u8>,
    pub headphone_volume: Option<u8>,
    pub monitor_mix: Option<u8>,
    pub pickup: Option<u8>,
    pub low_cut: Option<bool>,
    pub mid_boost: Option<bool>,
    /// EQ of the channel this report carries; reports alternate between the two.
    pub eq: Option<(EqChannel, Vec<EqPoint>)>,
}

/// Parses a state report starting at A5 (the report ID is already removed).
/// LED colours (three bytes each) sit before the EQ channel and its ten bands.
pub fn parse_input_report(bytes: &[u8], layout: &Layout, has_eq: bool) -> Result<DeviceState, &'static str> {
    if bytes.len() < 118 || bytes[..3] != [0xa5, 0x5a, 0xfe] {
        return Err("invalid report header or length");
    }
    let led_count = bytes[layout.led_count_at] as usize;
    let eq_channel_at = layout.led_count_at + 1 + 3 * led_count;
    let eq_start = eq_channel_at + 1;
    if eq_start + 100 >= bytes.len() {
        return Err("report truncated after LED data");
    }
    let read = |field: Option<Field>| -> Option<u8> {
        let index = match field? {
            Field::At(index) => index,
            Field::AfterEq(offset) => eq_start + 100 + offset - 1,
            Field::AfterLeds(index) if led_count == 0 => index.checked_sub(3 * MAX_LEDS)?,
            Field::AfterLeds(index) => index,
        };
        bytes.get(index).copied()
    };
    let flag = |field| read(field).map(|value| value != 0);
    let eq = if has_eq {
        let channel = match bytes[eq_channel_at] {
            0 => EqChannel::Output,
            1 => EqChannel::Microphone,
            _ => return Err("unknown EQ channel in report"),
        };
        let mut points = Vec::new();
        for offset in (eq_start..eq_start + 100).step_by(10) {
            let band = &bytes[offset..offset + 10];
            if band[0] == 0 {
                continue;
            }
            points.push(EqPoint {
                filter_type: u16::from_le_bytes([band[2], band[3]]),
                frequency_hz: u16::from_le_bytes([band[4], band[5]]),
                q: f32::from(u16::from_le_bytes([band[6], band[7]])) / 1024.0,
                gain_db: f32::from(i16::from_le_bytes([band[8], band[9]])) / 256.0,
            });
        }
        Some((channel, points))
    } else {
        None
    };
    Ok(DeviceState {
        light_on: flag(layout.light_switch),
        light_pattern: read(layout.light_pattern),
        mic_mute: flag(layout.mute),
        noise: read(layout.noise),
        voice: read(layout.voice),
        input_volume: read(layout.input_volume),
        headphone_volume: read(layout.headphone_volume),
        monitor_mix: read(layout.monitor_mix),
        pickup: read(layout.pickup),
        low_cut: flag(layout.low_cut),
        mid_boost: flag(layout.mid_boost),
        eq,
    })
}

/// Matches AM8Pro.js sendAM9Eq(): 10 filters, little-endian fields, 256-byte report.
pub fn eq_report(channel: EqChannel, points: &[EqPoint]) -> Result<Vec<u8>, &'static str> {
    if points.len() > 10 {
        return Err("EQ supports at most 10 points");
    }
    let mut bytes = vec![0, 0xa5, 0x5a, 0xfe, 0x66, 0x0a];
    bytes.push(match channel {
        EqChannel::Output => 0,
        EqChannel::Microphone => 1,
    });
    for point in points {
        if !(20..=20_000).contains(&point.frequency_hz)
            || point.filter_type > 6
            || !point.q.is_finite()
            || !(0.5..=10.0).contains(&point.q)
            || !point.gain_db.is_finite()
            || !(-12.0..=12.0).contains(&point.gain_db)
        {
            return Err("EQ point out of supported range");
        }
        let q = (point.q * 1024.0).round() as u16;
        let gain = ((point.gain_db * 256.0).round() as i16) as u16;
        bytes.extend_from_slice(&[1, 0]);
        bytes.extend_from_slice(&point.filter_type.to_le_bytes());
        bytes.extend_from_slice(&point.frequency_hz.to_le_bytes());
        bytes.extend_from_slice(&q.to_le_bytes());
        bytes.extend_from_slice(&gain.to_le_bytes());
    }
    for _ in points.len()..10 {
        bytes.extend_from_slice(&[0, 0, 1, 0, 0x2c, 1, 0xcd, 2, 0, 0]);
    }
    bytes.push(0x16);
    bytes.resize(257, 0); // report ID plus the 256-byte transfer in send.js
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::by_id;

    fn layout(id: &str) -> Layout {
        by_id(id).unwrap().layout.unwrap()
    }

    #[test]
    fn simple_packets_match_vendor_format() {
        assert_eq!(setting_report(0x02, 1), [0, 0xa5, 0x5a, 0xfe, 2, 2, 1, 0x16]);
        assert_eq!(reset_report(), [0, 0xa5, 0x5a, 0xfe, 1, 0x20, 0x16]);
    }

    #[test]
    fn frame_packet_matches_vendor_render() {
        let packet = frame_report(&[[0x12, 0x34, 0x56]; MAX_LEDS], 12);
        assert_eq!(packet.len(), 257);
        assert_eq!(&packet[..7], &[0, 0xa5, 0x5a, 0xfe, 0x26, 0x0b, 12]);
        assert_eq!(&packet[40..43], &[0x12, 0x34, 0x56]);
        assert_eq!(packet[43], 0x16);
        // A9: nine LEDs, as in Genie's A9S plugin (29, 0x0b, 9).
        let packet = frame_report(&[[1, 2, 3]; MAX_LEDS], 9);
        assert_eq!(&packet[..7], &[0, 0xa5, 0x5a, 0xfe, 29, 0x0b, 9]);
        assert_eq!(packet[7 + 27], 0x16);
    }

    #[test]
    fn eq_packet_has_correct_channel_and_size() {
        let points = [EqPoint { filter_type: 1, frequency_hz: 1000, q: 1.0, gain_db: -2.5 }];
        let packet = eq_report(EqChannel::Microphone, &points).unwrap();
        assert_eq!(packet.len(), 257);
        assert_eq!(&packet[..7], &[0, 0xa5, 0x5a, 0xfe, 0x66, 0x0a, 1]);
        assert_eq!(&packet[7..17], &[1, 0, 1, 0, 0xe8, 3, 0, 4, 0x80, 0xfd]);
        assert_eq!(packet[107], 0x16);
    }

    fn report() -> Vec<u8> {
        let mut report = vec![0; 256];
        report[..3].copy_from_slice(&[0xa5, 0x5a, 0xfe]);
        report[3] = 0x96;
        report
    }

    #[test]
    fn parses_am8_pro_report() {
        let mut bytes = report();
        bytes[4] = 5;
        bytes[5] = 1;
        bytes[12] = 27;
        bytes[14] = 12;
        bytes[15] = 12; // LEDs
        bytes[16 + 36] = 1; // microphone channel
        bytes[53..63].copy_from_slice(&[1, 0, 0, 0, 0xc8, 0, 0xd4, 2, 0, 0]);
        bytes[153] = 1; // light switch
        let state = parse_input_report(&bytes, &layout("am8pro"), true).unwrap();
        assert_eq!(state.light_pattern, Some(5));
        assert_eq!(state.mic_mute, Some(true));
        assert_eq!(state.input_volume, Some(27));
        assert_eq!(state.monitor_mix, Some(12));
        assert_eq!(state.light_on, Some(true));
        assert_eq!(state.pickup, None);
        let (channel, points) = state.eq.unwrap();
        assert_eq!(channel, EqChannel::Microphone);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].frequency_hz, 200);
        assert!((points[0].q - 0.707).abs() < 0.001);
    }

    #[test]
    fn led_count_shifts_everything_after_it() {
        let mut bytes = report();
        bytes[15] = 2;
        bytes[123] = 1;
        let state = parse_input_report(&bytes, &layout("am8pro"), true).unwrap();
        assert_eq!(state.eq.unwrap().0, EqChannel::Output);
        assert_eq!(state.light_on, Some(true));
    }

    #[test]
    fn a9_reads_pickup_pattern_after_the_eq() {
        let mut bytes = report();
        bytes[13] = 9; // LEDs
        bytes[10] = 3; // voice
        let after_eq = 13 + 27 + 1 + 100;
        bytes[after_eq + 1] = 1; // light switch
        bytes[after_eq + 2] = 4; // omni
        let state = parse_input_report(&bytes, &layout("a9"), true).unwrap();
        assert_eq!(state.voice, Some(3));
        assert_eq!(state.light_on, Some(true));
        assert_eq!(state.pickup, Some(4));
        assert_eq!(state.input_volume, None);
    }

    #[test]
    fn legacy_am8_offsets_move_when_no_leds_are_reported() {
        let mut bytes = report();
        bytes[15] = 12;
        bytes[154] = 2;
        bytes[161] = 7;
        let state = parse_input_report(&bytes, &layout("am8"), true).unwrap();
        assert_eq!(state.noise, Some(2));
        assert_eq!(state.monitor_mix, Some(7));

        let mut bytes = report();
        bytes[154 - 36] = 3;
        bytes[160 - 36] = 1;
        let state = parse_input_report(&bytes, &layout("am8"), true).unwrap();
        assert_eq!(state.noise, Some(3));
        assert_eq!(state.low_cut, Some(true));
    }
}
