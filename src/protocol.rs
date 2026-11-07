use serde::{Deserialize, Serialize};

pub const VENDOR_ID: u16 = 0x3142;
pub const PRODUCT_ID: u16 = 0x01a8;
pub const USAGE_PAGE: u16 = 0xff00;
pub const USAGE: u16 = 0x55aa;

#[derive(Debug, Clone, Copy)]
pub enum Setting {
    LightSwitch,
    NoiseSwitch,
    MicMute,
    VoiceChanger,
    RealTimeSwitch,
    LightPattern,
    InputVolume,
    HeadphoneVolume,
    MonitorMix,
    VolumeMode,
    Reset,
}

impl Setting {
    fn code(self) -> Option<u8> {
        match self {
            Setting::LightSwitch => Some(0x11),
            Setting::NoiseSwitch => Some(0x02),
            Setting::MicMute => Some(0x01),
            Setting::VoiceChanger => Some(0x04),
            Setting::RealTimeSwitch => Some(0x05),
            Setting::LightPattern => Some(0x00),
            Setting::InputVolume => Some(0x0d),
            Setting::HeadphoneVolume => Some(0x0e),
            Setting::MonitorMix => Some(0x0f),
            Setting::VolumeMode => Some(0x0c),
            Setting::Reset => None,
        }
    }
}

/// Report ID precedes the payload, matching AM8Pro.js configData().
pub fn setting_report(setting: Setting, value: u8) -> Vec<u8> {
    match setting.code() {
        Some(code) => vec![0, 0xa5, 0x5a, 0xfe, 0x02, code, value, 0x16],
        None => vec![0, 0xa5, 0x5a, 0xfe, 0x01, 0x20, 0x16],
    }
}

/// Number of RGB LEDs in the AM8 Pro ring.
pub const LED_COUNT: usize = 12;

/// One colour per LED, matching AM8Pro.js Render(): A5 5A FE 26 0B 0C, 12×RGB, 16.
/// Sent with report ID 0 and padded to the 256-byte transfer like the EQ report.
pub fn frame_report(frame: &[[u8; 3]; LED_COUNT]) -> Vec<u8> {
    let mut bytes = vec![0, 0xa5, 0x5a, 0xfe, 0x26, 0x0b, LED_COUNT as u8];
    for rgb in frame {
        bytes.extend_from_slice(rgb);
    }
    bytes.push(0x16);
    bytes.resize(257, 0);
    bytes
}

/// The same colour on every LED.
pub fn color_report(rgb: [u8; 3]) -> Vec<u8> {
    frame_report(&[rgb; LED_COUNT])
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

#[derive(Debug, Clone, PartialEq)]
pub struct Configuration {
    pub light_pattern: u8,
    pub light_on: bool,
    pub noise_reduction: bool,
    pub mic_mute: bool,
    pub voice_changer: u8,
    pub input_volume: u8,
    pub headphone_volume: u8,
    pub monitor_mix: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceState {
    pub configuration: Configuration,
    pub channel: EqChannel,
    pub eq_points: Vec<EqPoint>,
}

/// Genie helper's `receivedFromGetInputReport` data starts at A5, without report ID.
/// `led_count` inserts three bytes per LED before the EQ channel and filter data.
pub fn parse_input_report(bytes: &[u8]) -> Result<DeviceState, &'static str> {
    if bytes.len() < 118 || bytes[..3] != [0xa5, 0x5a, 0xfe] {
        return Err("invalid AM8 Pro report header or length");
    }
    let led_count = bytes[15] as usize;
    let eq_channel_at = 16 + 3 * led_count;
    let eq_start = eq_channel_at + 1;
    let light_switch_at = eq_start + 100;
    if light_switch_at >= bytes.len() {
        return Err("AM8 Pro report truncated after LED data");
    }
    let channel = match bytes[eq_channel_at] {
        0 => EqChannel::Output,
        1 => EqChannel::Microphone,
        _ => return Err("unknown EQ channel in AM8 Pro report"),
    };
    let mut eq_points = Vec::new();
    for offset in (eq_start..eq_start + 100).step_by(10) {
        let band = &bytes[offset..offset + 10];
        if band[0] == 0 {
            continue;
        }
        eq_points.push(EqPoint {
            filter_type: u16::from_le_bytes([band[2], band[3]]),
            frequency_hz: u16::from_le_bytes([band[4], band[5]]),
            q: f32::from(u16::from_le_bytes([band[6], band[7]])) / 1024.0,
            gain_db: f32::from(i16::from_le_bytes([band[8], band[9]])) / 256.0,
        });
    }
    Ok(DeviceState {
        configuration: Configuration {
            light_pattern: bytes[4],
            light_on: bytes[light_switch_at] != 0,
            noise_reduction: bytes[8] != 0,
            mic_mute: bytes[5] != 0,
            voice_changer: bytes[9],
            input_volume: bytes[12],
            headphone_volume: bytes[13],
            monitor_mix: bytes[14],
        },
        channel,
        eq_points,
    })
}

pub fn parse_helper_hex_report(values: &[String]) -> Result<DeviceState, &'static str> {
    let bytes = values
        .iter()
        .map(|value| u8::from_str_radix(value, 16).map_err(|_| "non-hex byte in helper report"))
        .collect::<Result<Vec<_>, _>>()?;
    parse_input_report(&bytes)
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

    #[test]
    fn simple_packet_matches_vendor_format() {
        assert_eq!(
            setting_report(Setting::NoiseSwitch, 1),
            [0, 0xa5, 0x5a, 0xfe, 2, 2, 1, 0x16]
        );
        assert_eq!(
            setting_report(Setting::Reset, 0),
            [0, 0xa5, 0x5a, 0xfe, 1, 0x20, 0x16]
        );
    }

    #[test]
    fn color_packet_matches_vendor_render() {
        let packet = color_report([0x12, 0x34, 0x56]);
        assert_eq!(packet.len(), 257);
        assert_eq!(&packet[..7], &[0, 0xa5, 0x5a, 0xfe, 0x26, 0x0b, 12]);
        assert_eq!(&packet[7..10], &[0x12, 0x34, 0x56]);
        assert_eq!(&packet[40..43], &[0x12, 0x34, 0x56]);
        assert_eq!(packet[43], 0x16);
    }

    #[test]
    fn eq_packet_has_correct_channel_and_size() {
        let points = [EqPoint {
            filter_type: 1,
            frequency_hz: 1000,
            q: 1.0,
            gain_db: -2.5,
        }];
        let packet = eq_report(EqChannel::Microphone, &points).unwrap();
        assert_eq!(packet.len(), 257);
        assert_eq!(&packet[..7], &[0, 0xa5, 0x5a, 0xfe, 0x66, 0x0a, 1]);
        assert_eq!(&packet[7..17], &[1, 0, 1, 0, 0xe8, 3, 0, 4, 0x80, 0xfd]);
        assert_eq!(packet[107], 0x16);
    }

    #[test]
    fn parses_observed_report_without_leading_report_id() {
        let mut report = vec![0; 256];
        report[..3].copy_from_slice(&[0xa5, 0x5a, 0xfe]);
        report[3] = 0x72;
        report[4] = 5;
        report[5] = 1;
        report[12] = 27;
        report[13] = 27;
        report[14] = 12;
        report[16] = 1;
        report[17..27].copy_from_slice(&[1, 0, 0, 0, 0xc8, 0, 0xd4, 2, 0, 0]);
        let state = parse_input_report(&report).unwrap();
        assert_eq!(state.channel, EqChannel::Microphone);
        assert_eq!(state.configuration.light_pattern, 5);
        assert!(state.configuration.mic_mute);
        assert_eq!(state.configuration.input_volume, 27);
        assert_eq!(state.eq_points.len(), 1);
        assert_eq!(state.eq_points[0].frequency_hz, 200);
        assert!((state.eq_points[0].q - 0.707).abs() < 0.001);
    }

    #[test]
    fn led_count_shifts_eq_and_light_switch() {
        let mut report = vec![0; 256];
        report[..3].copy_from_slice(&[0xa5, 0x5a, 0xfe]);
        report[15] = 2;
        report[22] = 0;
        report[123] = 1;
        let state = parse_input_report(&report).unwrap();
        assert_eq!(state.channel, EqChannel::Output);
        assert!(state.configuration.light_on);
    }
}
