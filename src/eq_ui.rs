use super::{AppWindow, EqBandView};
use fifine_am8_lite::{
    device::WriteCommand,
    editor::EqEditor,
    eq::{curve_path, export_genie_json},
    protocol::{EqChannel, EqPoint},
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use std::{
    rc::Rc,
    sync::{Arc, Mutex, mpsc::Sender},
    time::Duration,
};

const FILTERS: [u16; 5] = [0, 1, 2, 4, 5];

fn position(point: &EqPoint) -> (f32, f32) {
    (
        (f32::from(point.frequency_hz).max(20.0) / 20.0).log10() / 3.0,
        (12.0 - point.gain_db) / 24.0,
    )
}

pub fn refresh(ui: &AppWindow, editor: &EqEditor) {
    ui.set_eq_ready(ui.get_device_ready() && editor.loaded());
    ui.set_eq_channel_index(i32::from(editor.channel == EqChannel::Microphone));
    let names: Vec<SharedString> = editor
        .presets
        .names(editor.channel)
        .into_iter()
        .map(Into::into)
        .collect();
    ui.set_eq_preset_names(ModelRc::new(VecModel::from(names)));
    ui.set_eq_preset_index(editor.preset_index() as i32);
    ui.set_eq_locked(editor.locked);
    ui.set_eq_selected_band(editor.selected.map_or(-1, |index| index as i32));
    let bands: Vec<_> = editor
        .points()
        .iter()
        .map(|point| {
            let (plot_x, plot_y) = position(point);
            EqBandView {
                frequency: i32::from(point.frequency_hz),
                gain: point.gain_db,
                q: point.q,
                filter: i32::from(point.filter_type),
                plot_x,
                plot_y,
            }
        })
        .collect();
    ui.set_eq_bands(ModelRc::new(VecModel::from(bands)));
    ui.set_eq_curve(curve_path(editor.points()).into());
    if let Some(point) = editor.selected_point() {
        ui.set_selected_frequency(point.frequency_hz.to_string().into());
        ui.set_selected_q(format!("{:.3}", point.q).into());
        ui.set_selected_gain(format!("{:.2}", point.gain_db).into());
        ui.set_selected_filter(
            FILTERS
                .iter()
                .position(|kind| *kind == point.filter_type)
                .map_or(-1, |i| i as i32),
        );
    } else {
        ui.set_selected_frequency("".into());
        ui.set_selected_q("".into());
        ui.set_selected_gain("".into());
        ui.set_selected_filter(-1);
    }
}

struct Controls {
    ui: Weak<AppWindow>,
    editor: Arc<Mutex<EqEditor>>,
    writer: Sender<WriteCommand>,
    send_timers: [Timer; 2],
    lock_timer: Timer,
}

impl Controls {
    fn restart_lock_timer(&self) {
        let weak = self.ui.clone();
        let editor = self.editor.clone();
        self.lock_timer
            .start(TimerMode::SingleShot, Duration::from_secs(300), move || {
                let mut editor = editor.lock().unwrap();
                editor.locked = true;
                if let Some(ui) = weak.upgrade() {
                    ui.set_eq_locked(true);
                }
            });
    }

    fn edit(&self, action: impl FnOnce(&mut EqEditor) -> Result<(), String>) {
        let Some(ui) = self.ui.upgrade() else { return };
        if !ui.get_eq_ready() {
            return;
        }
        let mut editor = self.editor.lock().unwrap();
        match action(&mut editor) {
            Ok(()) => {
                ui.set_error_message("".into());
                let channel = editor.channel;
                let points = editor.points().to_vec();
                let writer = self.writer.clone();
                self.send_timers[usize::from(channel == EqChannel::Microphone)].start(
                    TimerMode::SingleShot,
                    Duration::from_millis(500),
                    move || {
                        let _ = writer.send(WriteCommand::Eq(channel, points.clone()));
                    },
                );
                if !editor.locked {
                    self.restart_lock_timer();
                }
            }
            Err(error) => ui.set_error_message(error.into()),
        }
        refresh(&ui, &editor);
    }
}

pub fn wire(ui: &AppWindow, editor: Arc<Mutex<EqEditor>>, writer: Sender<WriteCommand>) {
    refresh(ui, &editor.lock().unwrap());
    let controls = Rc::new(Controls {
        ui: ui.as_weak(),
        editor,
        writer,
        send_timers: [Timer::default(), Timer::default()],
        lock_timer: Timer::default(),
    });
    let state = controls.clone();
    ui.on_eq_channel_changed(move |index| {
        let mut editor = state.editor.lock().unwrap();
        editor.choose_channel(if index == 1 {
            EqChannel::Microphone
        } else {
            EqChannel::Output
        });
        if let Some(ui) = state.ui.upgrade() {
            refresh(&ui, &editor)
        }
    });
    let state = controls.clone();
    ui.on_eq_lock_changed(move |locked| {
        state.editor.lock().unwrap().locked = locked;
        if let Some(ui) = state.ui.upgrade() {
            ui.set_eq_locked(locked)
        }
        if locked {
            state.lock_timer.stop()
        } else {
            state.restart_lock_timer()
        }
    });
    let state = controls.clone();
    ui.on_eq_plot_pressed(move |x, y| {
        let mut editor = state.editor.lock().unwrap();
        editor.selected = editor
            .points()
            .iter()
            .enumerate()
            .filter_map(|(i, point)| {
                let (px, py) = position(point);
                let distance = ((px - x) * 2.5).powi(2) + (py - y).powi(2);
                (distance < 0.0025).then_some((i, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(index, _)| index);
        if let Some(ui) = state.ui.upgrade() {
            refresh(&ui, &editor)
        }
    });
    let state = controls.clone();
    ui.on_eq_plot_moved(move |x, y| {
        if state.editor.lock().unwrap().selected.is_none() {
            return;
        }
        state.edit(|editor| {
            let frequency = (20.0 * 1000f32.powf(x.clamp(0.0, 1.0))).round() as u16;
            let gain = ((12.0 - 24.0 * y.clamp(0.0, 1.0)) * 10.0).round() / 10.0;
            editor.move_selected(frequency, gain)
        });
    });
    let state = controls.clone();
    ui.on_eq_preset_changed(move |index| {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if index == state.editor.lock().unwrap().preset_index() {
            return;
        }
        state.edit(|editor| editor.choose_preset(index));
    });
    let state = controls.clone();
    ui.on_eq_field_changed(move |field, value| {
        let value = value.trim().replace(',', ".");
        state.edit(|editor| {
            let invalid = |_| "Введите число в допустимом диапазоне".to_owned();
            match field.as_str() {
                "frequency" => editor.edit_frequency(
                    value
                        .parse()
                        .map_err(|_| "Введите частоту от 20 до 20000 Гц")?,
                ),
                "gain" => editor.edit_gain(value.parse().map_err(invalid)?),
                "q" => editor.edit_q(value.parse().map_err(invalid)?),
                _ => Ok(()),
            }
        });
    });
    let state = controls.clone();
    ui.on_eq_filter_changed(move |index| {
        if let Some(&filter) = FILTERS.get(index as usize) {
            state.edit(|editor| editor.edit_filter(filter))
        }
    });
    let state = controls.clone();
    ui.on_eq_add_band(move || state.edit(EqEditor::add_band));
    let state = controls.clone();
    ui.on_eq_remove_band(move || state.edit(EqEditor::remove_selected));
    let state = controls.clone();
    ui.on_eq_reset(move || state.edit(EqEditor::reset_preset));
    let state = controls.clone();
    ui.on_eq_import(move || {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("FIFINE EQ", &["txt", "json"])
            .pick_file()
        else {
            return;
        };
        state.edit(|editor| {
            let content = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
            editor.import(&content)
        });
    });
    ui.on_eq_export(move || {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("FIFINE EQ", &["txt"])
            .set_file_name("fifine.txt")
            .save_file()
        else {
            return;
        };
        let result = export_genie_json(controls.editor.lock().unwrap().points())
            .and_then(|json| std::fs::write(path, json).map_err(|error| error.to_string()));
        if let Some(ui) = controls.ui.upgrade() {
            ui.set_error_message(result.err().unwrap_or_default().into());
        }
    });
}
