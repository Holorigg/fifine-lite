use crate::eq::{Presets, import_genie_json};
use crate::protocol::{EqChannel, EqPoint, eq_report};

#[derive(Default)]
struct ChannelState {
    current: Vec<EqPoint>,
    device_default: Option<Vec<EqPoint>>,
    preset_index: usize,
    base_preset_index: usize,
    custom: Option<Vec<EqPoint>>,
}

pub struct EqEditor {
    pub presets: Presets,
    channels: [ChannelState; 2],
    pub channel: EqChannel,
    pub selected: Option<usize>,
    pub locked: bool,
}

impl EqEditor {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            presets: Presets::load_bundled()?,
            channels: [ChannelState::default(), ChannelState::default()],
            channel: EqChannel::Microphone,
            selected: None,
            locked: true,
        })
    }

    fn index(channel: EqChannel) -> usize {
        match channel {
            EqChannel::Output => 0,
            EqChannel::Microphone => 1,
        }
    }

    fn active(&self) -> &ChannelState {
        &self.channels[Self::index(self.channel)]
    }
    fn active_mut(&mut self) -> &mut ChannelState {
        &mut self.channels[Self::index(self.channel)]
    }

    pub fn receive_device(&mut self, channel: EqChannel, points: Vec<EqPoint>) -> bool {
        let state = &mut self.channels[Self::index(channel)];
        if state.device_default.is_none() {
            state.device_default = Some(points.clone());
            state.current = points;
            return channel == self.channel;
        }
        false
    }

    pub fn loaded(&self) -> bool {
        self.active().device_default.is_some()
    }
    pub fn points(&self) -> &[EqPoint] {
        &self.active().current
    }
    pub fn preset_index(&self) -> usize {
        self.active().preset_index
    }
    pub fn selected_point(&self) -> Option<&EqPoint> {
        self.selected.and_then(|index| self.points().get(index))
    }

    pub fn choose_channel(&mut self, channel: EqChannel) {
        self.channel = channel;
        self.selected = None;
    }

    pub fn choose_preset(&mut self, index: usize) -> Result<(), String> {
        let custom = self.presets.custom_index(self.channel);
        let points = if index == 0 {
            self.active()
                .device_default
                .clone()
                .ok_or("Device Default ещё не прочитан")?
        } else if index == custom {
            self.active()
                .custom
                .clone()
                .ok_or("Сначала измените одну из точек EQ")?
        } else {
            self.presets.points(self.channel, index)?
        };
        eq_report(self.channel, &points).map_err(str::to_owned)?;
        let state = self.active_mut();
        state.current = points;
        state.preset_index = index;
        if index != custom {
            state.base_preset_index = index;
        }
        self.selected = None;
        Ok(())
    }

    pub fn reset_preset(&mut self) -> Result<(), String> {
        let index = self.active().base_preset_index;
        self.choose_preset(index)?;
        Ok(())
    }

    fn edit(&mut self, update: impl FnOnce(&mut Vec<EqPoint>)) -> Result<(), String> {
        if self.locked {
            return Err("Сначала снимите замок EQ".to_owned());
        }
        let mut next = self.points().to_vec();
        update(&mut next);
        eq_report(self.channel, &next).map_err(str::to_owned)?;
        let custom = self.presets.custom_index(self.channel);
        let state = self.active_mut();
        state.custom = Some(next.clone());
        state.current = next;
        state.preset_index = custom;
        Ok(())
    }

    pub fn edit_frequency(&mut self, frequency: u16) -> Result<(), String> {
        let index = self.selected.ok_or("Точка EQ не выбрана")?;
        self.edit(|points| points[index].frequency_hz = frequency)
    }

    pub fn edit_q(&mut self, q: f32) -> Result<(), String> {
        let index = self.selected.ok_or("Точка EQ не выбрана")?;
        self.edit(|points| points[index].q = q)
    }

    pub fn edit_gain(&mut self, gain: f32) -> Result<(), String> {
        let index = self.selected.ok_or("Точка EQ не выбрана")?;
        self.edit(|points| points[index].gain_db = gain)
    }

    pub fn edit_filter(&mut self, filter_type: u16) -> Result<(), String> {
        let index = self.selected.ok_or("Точка EQ не выбрана")?;
        self.edit(|points| points[index].filter_type = filter_type)
    }

    pub fn move_selected(&mut self, frequency: u16, gain: f32) -> Result<(), String> {
        let index = self.selected.ok_or("Точка EQ не выбрана")?;
        self.edit(|points| {
            points[index].frequency_hz = frequency;
            points[index].gain_db = gain;
        })
    }

    pub fn add_band(&mut self) -> Result<(), String> {
        if self.points().len() >= 10 {
            return Err("Максимум 10 точек EQ".to_owned());
        }
        let next_index = self.points().len();
        self.edit(|points| {
            points.push(EqPoint {
                filter_type: 0,
                frequency_hz: 1000,
                q: 0.707,
                gain_db: 0.0,
            })
        })?;
        self.selected = Some(next_index);
        Ok(())
    }

    pub fn remove_selected(&mut self) -> Result<(), String> {
        let index = self.selected.ok_or("Точка EQ не выбрана")?;
        self.edit(|points| {
            points.remove(index);
        })?;
        self.selected = None;
        Ok(())
    }

    pub fn import(&mut self, json: &str) -> Result<(), String> {
        let points = import_genie_json(json)?;
        self.edit(|current| *current = points)?;
        self.selected = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiving_both_channels_preserves_their_own_defaults() {
        let mut editor = EqEditor::new().unwrap();
        let output = vec![EqPoint {
            filter_type: 0,
            frequency_hz: 100,
            q: 0.707,
            gain_db: 1.0,
        }];
        let mic = vec![EqPoint {
            filter_type: 0,
            frequency_hz: 200,
            q: 0.707,
            gain_db: 2.0,
        }];
        editor.receive_device(EqChannel::Output, output.clone());
        editor.receive_device(EqChannel::Microphone, mic.clone());
        editor.choose_channel(EqChannel::Microphone);
        assert_eq!(editor.points(), mic);
        editor.choose_channel(EqChannel::Output);
        assert_eq!(editor.points(), output);
    }
}
