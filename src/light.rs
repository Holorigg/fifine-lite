//! Custom lighting for the 12-LED ring: static colours, per-LED colours and animations.
//! Frames are computed on the writer thread and streamed to the microphone.
use crate::protocol::LED_COUNT;

pub type Rgb = [u8; 3];
pub type Frame = [Rgb; LED_COUNT];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LightMode {
    /// Every LED shows its own colour (all equal for a single colour).
    Static(Frame),
    /// The colour fades in and out.
    Breath { color: Rgb, speed: f32 },
    /// A full hue wheel rotating around the ring.
    Rainbow { speed: f32 },
    /// Two colours blended around the ring, rotating.
    Gradient { from: Rgb, to: Rgb, speed: f32 },
    /// A VU ring: the number of lit LEDs follows the microphone level.
    Voice { color: Rgb },
}

impl LightMode {
    pub fn needs_level(&self) -> bool {
        matches!(self, LightMode::Voice { .. })
    }

    /// Frame at `t` seconds; `level` is the microphone peak 0–1.
    pub fn frame(&self, t: f32, level: f32) -> Frame {
        match *self {
            LightMode::Static(frame) => frame,
            LightMode::Breath { color, speed } => {
                let phase = (t * speed / 3.0) * std::f32::consts::TAU;
                let brightness = 0.12 + 0.88 * (0.5 - 0.5 * phase.cos());
                [scale(color, brightness); LED_COUNT]
            }
            LightMode::Rainbow { speed } => {
                std::array::from_fn(|i| hsv_to_rgb([(i as f32 / LED_COUNT as f32 + t * speed * 0.15) * 360.0, 1.0, 1.0]))
            }
            LightMode::Gradient { from, to, speed } => std::array::from_fn(|i| {
                let pos = (i as f32 / LED_COUNT as f32 + t * speed * 0.1).rem_euclid(1.0);
                // Triangle wave so the ring loops without a seam.
                let k = 1.0 - (pos * 2.0 - 1.0).abs();
                mix(from, to, k)
            }),
            LightMode::Voice { color } => {
                // Map -50..-6 dBFS to 0..12 LEDs; the last lit LED is partially bright.
                let db = if level <= 0.0001 { -60.0 } else { 20.0 * level.log10() };
                let lit = ((db + 50.0) / 44.0).clamp(0.0, 1.0) * LED_COUNT as f32;
                std::array::from_fn(|i| {
                    let fill = (lit - i as f32).clamp(0.0, 1.0);
                    scale(color, 0.06 + 0.94 * fill)
                })
            }
        }
    }
}

fn scale([r, g, b]: Rgb, k: f32) -> Rgb {
    let f = |v: u8| (f32::from(v) * k).round().clamp(0.0, 255.0) as u8;
    [f(r), f(g), f(b)]
}

fn mix(a: Rgb, b: Rgb, k: f32) -> Rgb {
    std::array::from_fn(|i| (f32::from(a[i]) * (1.0 - k) + f32::from(b[i]) * k).round() as u8)
}

/// HSV (hue in degrees, saturation and value 0–1) to 8-bit RGB.
pub fn hsv_to_rgb([hue, sat, value]: [f32; 3]) -> Rgb {
    let h = hue.rem_euclid(360.0) / 60.0;
    let v = value.clamp(0.0, 1.0);
    let c = v * sat.clamp(0.0, 1.0);
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to_byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [to_byte(r), to_byte(g), to_byte(b)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_conversion_hits_primaries() {
        assert_eq!(hsv_to_rgb([0.0, 1.0, 1.0]), [255, 0, 0]);
        assert_eq!(hsv_to_rgb([120.0, 1.0, 1.0]), [0, 255, 0]);
        assert_eq!(hsv_to_rgb([240.0, 1.0, 1.0]), [0, 0, 255]);
        assert_eq!(hsv_to_rgb([0.0, 0.0, 1.0]), [255, 255, 255]);
        assert_eq!(hsv_to_rgb([300.0, 1.0, 0.0]), [0, 0, 0]);
    }

    #[test]
    fn voice_ring_fills_with_level() {
        let mode = LightMode::Voice { color: [255, 0, 0] };
        let silent = mode.frame(0.0, 0.0);
        let loud = mode.frame(0.0, 1.0);
        assert!(silent.iter().all(|led| led[0] < 20));
        assert!(loud.iter().all(|led| led[0] == 255));
    }

    #[test]
    fn breath_stays_within_colour() {
        let mode = LightMode::Breath { color: [200, 100, 50], speed: 1.0 };
        for step in 0..30 {
            let frame = mode.frame(step as f32 * 0.1, 0.0);
            assert!(frame[0][0] <= 200 && frame[0][1] <= 100);
        }
    }
}
