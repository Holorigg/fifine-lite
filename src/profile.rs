//! What the app knows about each supported microphone: USB identity, command codes,
//! where each setting sits in the state report and which controls to show.
//!
//! Offsets and codes come from fifine Genie's device tables. Only the AM8 Pro has been
//! checked against real hardware; the rest are marked `verified: false`.

/// All supported microphones share fifine's USB vendor ID.
pub const VENDOR_ID: u16 = 0x3142;
/// Vendor-defined HID collection the control interface normally uses.
pub const USAGE_PAGE: u16 = 0xff00;
pub const USAGE: u16 = 0x55aa;

/// The most LEDs any supported ring has.
pub const MAX_LEDS: usize = 12;

/// Position of a one-byte value in the state report (index 0 is the leading A5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Field {
    /// Fixed index.
    At(usize),
    /// Counted from the end of the EQ block, which follows the per-LED colours.
    AfterEq(usize),
    /// Fixed index for a full LED block; moves down when the report carries no LED colours.
    AfterLeds(usize),
}

/// Where the settings are in the state report.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// Index of the "number of LEDs in this report" byte; LED colours, the EQ channel
    /// and ten EQ bands follow it.
    pub led_count_at: usize,
    pub light_switch: Option<Field>,
    pub light_pattern: Option<Field>,
    pub mute: Option<Field>,
    pub noise: Option<Field>,
    pub voice: Option<Field>,
    pub input_volume: Option<Field>,
    pub headphone_volume: Option<Field>,
    pub monitor_mix: Option<Field>,
    pub pickup: Option<Field>,
    pub low_cut: Option<Field>,
    pub mid_boost: Option<Field>,
}

/// Command codes (the byte after `A5 5A FE 02`). `None` means the microphone has no such setting.
#[derive(Debug, Clone, Copy)]
pub struct Codes {
    pub light_switch: Option<u8>,
    pub light_pattern: Option<u8>,
    pub mute: Option<u8>,
    pub noise: Option<u8>,
    pub voice: Option<u8>,
    /// Sent after a voice change so the effect is heard live.
    pub realtime: Option<u8>,
    pub input_volume: Option<u8>,
    pub headphone_volume: Option<u8>,
    pub monitor_mix: Option<u8>,
    /// Selects which of the three volumes the next value applies to.
    pub volume_mode: Option<u8>,
    pub pickup: Option<u8>,
    pub low_cut: Option<u8>,
    pub mid_boost: Option<u8>,
}

#[derive(Debug)]
pub struct Profile {
    /// Stable key used in the settings file.
    pub id: &'static str,
    pub name: &'static str,
    pub product_id: u16,
    /// Checked on real hardware.
    pub verified: bool,
    /// LEDs in the ring; 0 means no controllable lighting.
    pub leds: usize,
    /// Number of built-in light effects; 0 means they cannot be chosen from the computer.
    pub patterns: u8,
    /// Selecting pattern 16 makes the last colour frame stay without a stream of frames.
    pub holds_custom_light: bool,
    /// Highest noise suppression level: 1 is an on/off switch.
    pub noise_levels: u8,
    /// Highest input and headphone volume step, and highest monitor mix step.
    pub volume_max: u8,
    pub mix_max: u8,
    pub has_eq: bool,
    pub has_reset: bool,
    pub codes: Codes,
    /// `None`: the microphone only takes commands and lighting, its state cannot be read.
    pub layout: Option<Layout>,
}

const NO_CODES: Codes = Codes {
    light_switch: None,
    light_pattern: None,
    mute: None,
    noise: None,
    voice: None,
    realtime: None,
    input_volume: None,
    headphone_volume: None,
    monitor_mix: None,
    volume_mode: None,
    pickup: None,
    low_cut: None,
    mid_boost: None,
};

const NO_LAYOUT: Layout = Layout {
    led_count_at: 0,
    light_switch: None,
    light_pattern: None,
    mute: None,
    noise: None,
    voice: None,
    input_volume: None,
    headphone_volume: None,
    monitor_mix: None,
    pickup: None,
    low_cut: None,
    mid_boost: None,
};

/// Commands shared by the AM8 Pro, AM9, A9 and A8 Plus.
const MODERN_CODES: Codes = Codes {
    light_switch: Some(0x11),
    light_pattern: Some(0x00),
    mute: Some(0x01),
    noise: Some(0x02),
    voice: Some(0x04),
    realtime: Some(0x05),
    ..NO_CODES
};

/// Commands of the original AM8 and the AM68.
const LEGACY_CODES: Codes = Codes {
    light_switch: Some(0x00),
    mute: Some(0x01),
    noise: Some(0x12),
    monitor_mix: Some(0x0f),
    volume_mode: Some(0x0c),
    ..NO_CODES
};

/// Profiles are compared by identity: there is one per model.
impl PartialEq for Profile {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

pub static PROFILES: [Profile; 6] = [
    Profile {
        id: "am8pro",
        name: "AM8 Pro",
        product_id: 0x01a8,
        verified: true,
        leds: 12,
        patterns: 15,
        holds_custom_light: true,
        noise_levels: 1,
        volume_max: 27,
        mix_max: 24,
        has_eq: true,
        has_reset: true,
        codes: Codes {
            input_volume: Some(0x0d),
            headphone_volume: Some(0x0e),
            monitor_mix: Some(0x0f),
            volume_mode: Some(0x0c),
            ..MODERN_CODES
        },
        layout: Some(Layout {
            led_count_at: 15,
            light_switch: Some(Field::AfterEq(1)),
            light_pattern: Some(Field::At(4)),
            mute: Some(Field::At(5)),
            noise: Some(Field::At(8)),
            voice: Some(Field::At(9)),
            input_volume: Some(Field::At(12)),
            headphone_volume: Some(Field::At(13)),
            monitor_mix: Some(Field::At(14)),
            ..NO_LAYOUT
        }),
    },
    Profile {
        id: "am9",
        name: "AM9",
        product_id: 0xa009,
        verified: false,
        leds: 12,
        patterns: 15,
        holds_custom_light: false,
        noise_levels: 1,
        volume_max: 0,
        mix_max: 0,
        has_eq: true,
        has_reset: true,
        codes: MODERN_CODES,
        layout: Some(Layout {
            led_count_at: 11,
            light_switch: Some(Field::AfterEq(1)),
            light_pattern: Some(Field::At(4)),
            mute: Some(Field::At(5)),
            noise: Some(Field::At(8)),
            voice: Some(Field::At(9)),
            ..NO_LAYOUT
        }),
    },
    Profile {
        id: "a9",
        name: "A9",
        product_id: 0x02a9,
        verified: false,
        leds: 9,
        patterns: 15,
        holds_custom_light: false,
        noise_levels: 1,
        volume_max: 0,
        mix_max: 0,
        has_eq: true,
        has_reset: true,
        codes: Codes { pickup: Some(0x12), ..MODERN_CODES },
        layout: Some(Layout {
            led_count_at: 13,
            light_switch: Some(Field::AfterEq(1)),
            light_pattern: Some(Field::At(4)),
            mute: Some(Field::At(5)),
            noise: Some(Field::At(8)),
            voice: Some(Field::At(10)),
            pickup: Some(Field::AfterEq(2)),
            ..NO_LAYOUT
        }),
    },
    // Genie drives only the lighting of the A8 Plus and never reads its state.
    Profile {
        id: "a8plus",
        name: "A8 Plus",
        product_id: 0x0a80,
        verified: false,
        leds: 10,
        patterns: 0,
        holds_custom_light: false,
        noise_levels: 0,
        volume_max: 0,
        mix_max: 0,
        has_eq: false,
        has_reset: false,
        codes: NO_CODES,
        layout: None,
    },
    Profile {
        id: "am8",
        name: "AM8",
        product_id: 0xa010,
        verified: false,
        leds: 12,
        patterns: 0,
        holds_custom_light: false,
        noise_levels: 3,
        volume_max: 0,
        mix_max: 24,
        has_eq: true,
        has_reset: true,
        codes: Codes { low_cut: Some(0x16), mid_boost: Some(0x15), ..LEGACY_CODES },
        layout: Some(Layout {
            led_count_at: 15,
            light_switch: Some(Field::At(4)),
            mute: Some(Field::At(5)),
            noise: Some(Field::AfterLeds(154)),
            low_cut: Some(Field::AfterLeds(160)),
            mid_boost: Some(Field::AfterLeds(159)),
            monitor_mix: Some(Field::AfterLeds(161)),
            ..NO_LAYOUT
        }),
    },
    Profile {
        id: "am68",
        name: "AM68",
        product_id: 0x0a68,
        verified: false,
        leds: 12,
        patterns: 0,
        holds_custom_light: false,
        noise_levels: 3,
        volume_max: 27,
        mix_max: 24,
        has_eq: true,
        has_reset: true,
        codes: Codes { input_volume: Some(0x0d), headphone_volume: Some(0x0e), ..LEGACY_CODES },
        layout: Some(Layout {
            led_count_at: 15,
            light_switch: Some(Field::At(4)),
            mute: Some(Field::At(5)),
            noise: Some(Field::AfterLeds(154)),
            input_volume: Some(Field::At(12)),
            headphone_volume: Some(Field::At(13)),
            monitor_mix: Some(Field::At(14)),
            ..NO_LAYOUT
        }),
    },
];

pub fn by_product_id(product_id: u16) -> Option<&'static Profile> {
    PROFILES.iter().find(|profile| profile.product_id == product_id)
}

pub fn by_id(id: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|profile| profile.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_distinct_and_fit_the_ring() {
        for (index, profile) in PROFILES.iter().enumerate() {
            assert!(profile.leds <= MAX_LEDS, "{}", profile.id);
            for other in &PROFILES[index + 1..] {
                assert_ne!(profile.id, other.id);
                assert_ne!(profile.product_id, other.product_id);
            }
        }
        assert_eq!(by_product_id(424).map(|p| p.id), Some("am8pro"));
        assert_eq!(by_product_id(40976).map(|p| p.id), Some("am8"));
    }
}
