use crate::protocol::{EqChannel, EqPoint};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeniePoint {
    pub x: f32,
    pub y: f32,
    pub q: f32,
    #[serde(rename = "type")]
    pub filter_type: u16,
}

impl TryFrom<GeniePoint> for EqPoint {
    type Error = &'static str;

    fn try_from(value: GeniePoint) -> Result<Self, Self::Error> {
        if !value.x.is_finite()
            || !(20.0..=20_000.0).contains(&value.x)
            || !value.y.is_finite()
            || !(-12.0..=12.0).contains(&value.y)
            || !value.q.is_finite()
            || !(0.5..=10.0).contains(&value.q)
            || value.filter_type > 6
        {
            return Err("EQ point outside AM8 Pro range");
        }
        Ok(Self {
            filter_type: value.filter_type,
            frequency_hz: value.x.round() as u16,
            gain_db: value.y,
            q: value.q,
        })
    }
}

impl From<&EqPoint> for GeniePoint {
    fn from(value: &EqPoint) -> Self {
        Self {
            x: value.frequency_hz as f32,
            y: value.gain_db,
            q: value.q,
            filter_type: value.filter_type,
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct Preset {
    pub label: String,
    #[serde(rename = "presetsValue")]
    points: Vec<GeniePoint>,
}

#[derive(Deserialize)]
struct PresetFile {
    output: Vec<Preset>,
    microphone: Vec<Preset>,
}

pub struct Presets {
    output: Vec<Preset>,
    microphone: Vec<Preset>,
}

impl Presets {
    pub fn load_bundled() -> Result<Self, String> {
        let file: PresetFile = serde_json::from_str(include_str!("../data/presets.json"))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            output: file.output,
            microphone: file.microphone,
        })
    }

    pub fn names(&self, channel: EqChannel) -> Vec<String> {
        let source = self.source(channel);
        source
            .iter()
            .enumerate()
            .map(|(index, preset)| {
                if index == 0 {
                    "Device Default".to_owned()
                } else {
                    preset.label.clone()
                }
            })
            .chain(std::iter::once("Custom".to_owned()))
            .collect()
    }

    pub fn points(&self, channel: EqChannel, index: usize) -> Result<Vec<EqPoint>, String> {
        let preset = self.source(channel).get(index).ok_or("unknown EQ preset")?;
        preset
            .points
            .iter()
            .cloned()
            .map(|point| EqPoint::try_from(point).map_err(str::to_owned))
            .collect()
    }

    pub fn custom_index(&self, channel: EqChannel) -> usize {
        self.source(channel).len()
    }

    fn source(&self, channel: EqChannel) -> &[Preset] {
        match channel {
            EqChannel::Output => &self.output,
            EqChannel::Microphone => &self.microphone,
        }
    }
}

pub fn import_genie_json(text: &str) -> Result<Vec<EqPoint>, String> {
    let data: Vec<GeniePoint> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if data.len() > 10 {
        return Err("EQ supports at most 10 points".to_owned());
    }
    data.into_iter()
        .map(|point| EqPoint::try_from(point).map_err(str::to_owned))
        .collect()
}

pub fn export_genie_json(points: &[EqPoint]) -> Result<String, String> {
    let data: Vec<_> = points.iter().map(GeniePoint::from).collect();
    serde_json::to_string_pretty(&data).map_err(|e| e.to_string())
}

// Genie's 44.1 kHz RBJ biquad response, used only to draw the curve.
fn band_response_db(point: &EqPoint, frequency_hz: f64) -> f64 {
    let w0 = 2.0 * std::f64::consts::PI * point.frequency_hz as f64 / 44_100.0;
    let cos = w0.cos();
    let alpha = w0.sin() / (2.0 * point.q as f64);
    let a = 10f64.powf(point.gain_db as f64 / 40.0);
    let root_a = a.sqrt();
    let (b0, b1, b2, a0, a1, a2) = match point.filter_type {
        1 => (
            a * (a + 1.0 - (a - 1.0) * cos + 2.0 * root_a * alpha),
            2.0 * a * (a - 1.0 - (a + 1.0) * cos),
            a * (a + 1.0 - (a - 1.0) * cos - 2.0 * root_a * alpha),
            a + 1.0 + (a - 1.0) * cos + 2.0 * root_a * alpha,
            -2.0 * (a - 1.0 + (a + 1.0) * cos),
            a + 1.0 + (a - 1.0) * cos - 2.0 * root_a * alpha,
        ),
        2 => (
            a * (a + 1.0 + (a - 1.0) * cos + 2.0 * root_a * alpha),
            -2.0 * a * (a - 1.0 + (a + 1.0) * cos),
            a * (a + 1.0 + (a - 1.0) * cos - 2.0 * root_a * alpha),
            a + 1.0 - (a - 1.0) * cos + 2.0 * root_a * alpha,
            2.0 * (a - 1.0 - (a + 1.0) * cos),
            a + 1.0 - (a - 1.0) * cos - 2.0 * root_a * alpha,
        ),
        3 => (
            (1.0 - cos) / 2.0,
            1.0 - cos,
            (1.0 - cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        ),
        4 => (
            (1.0 + cos) / 2.0,
            -(1.0 + cos),
            (1.0 + cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        ),
        6 => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        _ => (
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        ),
    };
    let w = 2.0 * std::f64::consts::PI * frequency_hz / 44_100.0;
    let re_b = b0 + b1 * w.cos() + b2 * (2.0 * w).cos();
    let im_b = -b1 * w.sin() - b2 * (2.0 * w).sin();
    let re_a = a0 + a1 * w.cos() + a2 * (2.0 * w).cos();
    let im_a = -a1 * w.sin() - a2 * (2.0 * w).sin();
    let numerator = re_b * re_b + im_b * im_b;
    let denominator = re_a * re_a + im_a * im_a;
    if denominator <= 0.0 || numerator <= 0.0 {
        return -120.0;
    }
    10.0 * (numerator / denominator).log10()
}

pub fn curve_path(points: &[EqPoint]) -> String {
    let mut path = String::with_capacity(2200);
    for i in 0..=200 {
        let f = 20.0 * 1000f64.powf(i as f64 / 200.0);
        let db = points
            .iter()
            .map(|point| band_response_db(point, f))
            .sum::<f64>()
            .clamp(-12.0, 12.0);
        let x = i * 5;
        let y = ((12.0 - db) / 24.0 * 400.0).round() as i32;
        if i == 0 {
            path.push_str(&format!("M {x} {y}"));
        } else {
            path.push_str(&format!(" L {x} {y}"));
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_presets_are_available() {
        let presets = Presets::load_bundled().unwrap();
        assert_eq!(presets.names(EqChannel::Output).len(), 32);
        assert_eq!(presets.names(EqChannel::Microphone).len(), 9);
        for channel in [EqChannel::Output, EqChannel::Microphone] {
            for index in 0..presets.custom_index(channel) {
                let points = presets.points(channel, index).unwrap();
                crate::protocol::eq_report(channel, &points).unwrap();
            }
        }
    }

    #[test]
    fn genie_json_round_trip() {
        let points = vec![EqPoint {
            filter_type: 0,
            frequency_hz: 1000,
            q: 0.707,
            gain_db: -3.5,
        }];
        assert_eq!(
            import_genie_json(&export_genie_json(&points).unwrap()).unwrap(),
            points
        );
    }

    #[test]
    fn flat_eq_is_centered() {
        let path = curve_path(&[]);
        assert!(path.starts_with("M 0 200 L 5 200"));
    }
}
