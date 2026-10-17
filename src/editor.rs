//! Editor in the spirit of an Ableton Live device: flat dark panel, thin-arc dials
//! with the value underneath, a switchable Transfer / Activity display and
//! GR + output meters.

use std::collections::VecDeque;
use std::sync::Arc;

use egui::{
    Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use truce::prelude::*;
use truce_core::editor::{PluginContext, PluginContextReadF32};
use truce_egui::EguiEditor;
use truce_params::Params;

use crate::GersicPressureParams;
use crate::GersicPressureParamsParamId as P;
use crate::dsp::{SC_HPF_OFF_HZ, THRESHOLD_DB, static_curve_db};

const WIDTH: u32 = 600;
const HEIGHT: u32 = 236;
const HEADER_H: f32 = 22.0;

// Palette modelled on Live's dark theme.
const BG: Color32 = Color32::from_rgb(40, 40, 40);
const HEADER: Color32 = Color32::from_rgb(54, 54, 54);
const DISPLAY: Color32 = Color32::from_rgb(22, 22, 22);
const GRID: Color32 = Color32::from_rgb(46, 46, 46);
const TEXT: Color32 = Color32::from_rgb(210, 210, 210);
const TEXT_DIM: Color32 = Color32::from_rgb(130, 130, 130);
const TRACK: Color32 = Color32::from_rgb(78, 78, 78);
const ACCENT: Color32 = Color32::from_rgb(255, 160, 40);
const CURVE: Color32 = Color32::from_rgb(90, 200, 250);
const METER_LOW: Color32 = Color32::from_rgb(70, 200, 90);
const METER_MID: Color32 = Color32::from_rgb(230, 210, 60);
const METER_HIGH: Color32 = Color32::from_rgb(235, 70, 60);

/// Display range of the transfer and activity views, in dBFS.
const DB_MIN: f32 = -72.0;
const DB_MAX: f32 = 6.0;
/// GR meter / activity range, in dB.
const GR_RANGE: f32 = 24.0;
/// About four seconds of history at 60 fps.
const HISTORY_LEN: usize = 240;

#[derive(Clone, Copy, PartialEq)]
enum View {
    Transfer,
    Activity,
}

/// Per-editor UI state (meter ballistics, activity history, selected view).
struct Ui {
    view: View,
    out_peak: [f32; 2],
    gr_db: [f32; 2],
    env_db: f32,
    /// `(detector level dB, mix gain reduction dB)` per frame.
    history: VecDeque<(f32, f32)>,
}

pub(crate) fn create(params: Arc<GersicPressureParams>) -> Box<dyn Editor> {
    let mut state = Ui {
        view: View::Transfer,
        out_peak: [0.0; 2],
        gr_db: [0.0; 2],
        env_db: DB_MIN,
        history: VecDeque::with_capacity(HISTORY_LEN),
    };
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    EguiEditor::new(
        params,
        (WIDTH, HEIGHT),
        move |ui: &mut egui::Ui, cx: &PluginContext<GersicPressureParams>| {
            state.draw(ui, cx);
        },
    )
    .with_visuals(visuals)
    .into_editor()
}

fn db(lin: f32) -> f32 {
    if lin > 1e-9 {
        20.0 * lin.log10()
    } else {
        -180.0
    }
}

impl Ui {
    fn draw(&mut self, ui: &mut egui::Ui, cx: &PluginContext<GersicPressureParams>) {
        ui.ctx().request_repaint();
        let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.1);
        self.update_meters(cx, dt);

        let full = ui.max_rect();
        let painter = ui.painter_at(full);
        painter.rect_filled(full, 0.0, BG);

        // Title bar
        let header = Rect::from_min_size(full.min, vec2(full.width(), HEADER_H));
        painter.rect_filled(header, 0.0, HEADER);
        painter.circle_filled(pos2(header.left() + 12.0, header.center().y), 4.5, ACCENT);
        painter.text(
            pos2(header.left() + 24.0, header.center().y),
            Align2::LEFT_CENTER,
            "Gersic Pressure",
            FontId::proportional(12.0),
            TEXT,
        );
        painter.text(
            pos2(header.right() - 10.0, header.center().y),
            Align2::RIGHT_CENTER,
            "upward compressor",
            FontId::proportional(10.0),
            TEXT_DIM,
        );

        let body = Rect::from_min_max(
            pos2(full.left() + 10.0, header.bottom() + 10.0),
            full.max - vec2(10.0, 10.0),
        );

        // Left: display with view tabs.
        let display_w = 230.0;
        let tabs = Rect::from_min_size(body.min, vec2(display_w, 16.0));
        self.view_tabs(ui, tabs);
        let display = Rect::from_min_max(
            pos2(body.left(), tabs.bottom() + 4.0),
            pos2(body.left() + display_w, body.bottom()),
        );
        painter.rect_filled(display, 2.0, DISPLAY);
        match self.view {
            View::Transfer => self.draw_transfer(&painter, display, cx),
            View::Activity => self.draw_activity(&painter, display),
        }

        // Right: meters.
        let meters = Rect::from_min_max(pos2(body.right() - 74.0, body.top()), body.max);
        self.draw_meters(&painter, meters);

        // Centre: three main dials, then the v2 "extras" row.
        let centre = Rect::from_min_max(
            pos2(display.right() + 14.0, body.top()),
            pos2(meters.left() - 14.0, body.bottom()),
        );
        let main_w = centre.width() / 3.0;
        for (i, (id, label)) in [
            (P::Lookahead, "LookAhead"),
            (P::Amount, "Amount"),
            (P::Gain, "Out Gain"),
        ]
        .into_iter()
        .enumerate()
        {
            let r = Rect::from_min_size(
                pos2(centre.left() + main_w * i as f32, centre.top()),
                vec2(main_w, 100.0),
            );
            dial(ui, cx, r, id, label, 24.0);
        }

        let sep_y = centre.top() + 106.0;
        painter.hline(centre.x_range(), sep_y, Stroke::new(1.0_f32, GRID));

        let small_w = centre.width() / 4.0;
        let row_top = sep_y + 6.0;
        for (i, (id, label)) in [(P::Color, "Color"), (P::Link, "Link"), (P::ScHpf, "SC HPF")]
            .into_iter()
            .enumerate()
        {
            let r = Rect::from_min_size(
                pos2(centre.left() + small_w * i as f32, row_top),
                vec2(small_w, 76.0),
            );
            dial(ui, cx, r, id, label, 15.0);
        }
        let hq = Rect::from_center_size(
            pos2(centre.left() + small_w * 3.5, row_top + 34.0),
            vec2(40.0, 18.0),
        );
        toggle(ui, cx, hq, P::Hq, "HQ", "4x oversampled saturation");
    }

    fn update_meters(&mut self, cx: &PluginContext<GersicPressureParams>, dt: f32) {
        // Peak meters: instant attack, ~20 dB/s fall.
        let fall = 20.0 * dt;
        for (ch, id) in [P::MeterLeft, P::MeterRight].into_iter().enumerate() {
            let v = db(cx.get_meter(id)).max(DB_MIN);
            self.out_peak[ch] = if v > self.out_peak[ch] - fall {
                v
            } else {
                self.out_peak[ch] - fall
            };
        }
        // GR of the actual blend (what you hear), instant attack, ~40 dB/s return.
        let amount = cx.get_param_plain(P::Amount) * 0.01;
        for (ch, id) in [P::MeterGr, P::MeterGrRight].into_iter().enumerate() {
            let g = cx.get_meter(id).clamp(0.0, 1.0);
            let gr = -db(1.0 - amount + amount * g).min(0.0);
            self.gr_db[ch] = if gr > self.gr_db[ch] {
                gr
            } else {
                (self.gr_db[ch] - 40.0 * dt).max(gr)
            };
        }
        self.env_db = db(cx.get_meter(P::MeterEnv)).clamp(DB_MIN, DB_MAX);

        if self.history.len() == HISTORY_LEN {
            self.history.pop_front();
        }
        self.history
            .push_back((self.env_db, self.gr_db[0].max(self.gr_db[1])));
    }

    fn view_tabs(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter();
        let w = rect.width() / 2.0;
        for (i, (view, label)) in [(View::Transfer, "Transfer"), (View::Activity, "Activity")]
            .into_iter()
            .enumerate()
        {
            let r = Rect::from_min_size(
                pos2(rect.left() + w * i as f32, rect.top()),
                vec2(w - 2.0, rect.height()),
            );
            let resp = ui.interact(r, ui.id().with(("view", i)), Sense::click());
            if resp.clicked() {
                self.view = view;
            }
            let active = self.view == view;
            painter.rect_filled(r, 1.0, if active { ACCENT } else { HEADER });
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(10.0),
                if active { Color32::BLACK } else { TEXT_DIM },
            );
        }
    }

    fn draw_transfer(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        cx: &PluginContext<GersicPressureParams>,
    ) {
        let plot = rect.shrink(8.0);
        let to_x = |d: f32| plot.left() + (d - DB_MIN) / (DB_MAX - DB_MIN) * plot.width();
        let to_y = |d: f32| {
            plot.bottom() - (d.clamp(DB_MIN, DB_MAX) - DB_MIN) / (DB_MAX - DB_MIN) * plot.height()
        };

        for d in (-60..=0).step_by(12) {
            let d = d as f32;
            painter.vline(to_x(d), plot.y_range(), Stroke::new(1.0_f32, GRID));
            painter.hline(plot.x_range(), to_y(d), Stroke::new(1.0_f32, GRID));
        }
        painter.line_segment(
            [
                pos2(to_x(DB_MIN), to_y(DB_MIN)),
                pos2(to_x(DB_MAX), to_y(DB_MAX)),
            ],
            Stroke::new(1.0_f32, TRACK),
        );
        painter.extend(Shape::dashed_line(
            &[
                pos2(to_x(THRESHOLD_DB), plot.top()),
                pos2(to_x(THRESHOLD_DB), plot.bottom()),
            ],
            Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.6)),
            3.0,
            3.0,
        ));

        let amount = cx.get_param_plain(P::Amount) * 0.01;
        let out_gain = 10f32.powf(cx.get_param_plain(P::Gain) / 20.0);
        let clip = cx.get_param_plain(P::Color) * 0.01;
        let curve = |input_db: f32| {
            let m = 10f32.powf(static_curve_db(input_db, amount) / 20.0) * out_gain;
            db(m / (1.0 + clip * m))
        };

        let points: Vec<Pos2> = (0..=96)
            .map(|i| {
                let d = DB_MIN + (DB_MAX - DB_MIN) * i as f32 / 96.0;
                pos2(to_x(d), to_y(curve(d)))
            })
            .collect();
        painter.line(points, Stroke::new(2.0_f32, CURVE));

        if self.env_db > DB_MIN + 0.5 {
            painter.circle_filled(
                pos2(to_x(self.env_db), to_y(curve(self.env_db))),
                4.0,
                ACCENT,
            );
        }
        painter.text(
            pos2(plot.left() + 2.0, plot.top()),
            Align2::LEFT_TOP,
            "out",
            FontId::proportional(9.0),
            TEXT_DIM,
        );
        painter.text(
            plot.right_bottom() - vec2(2.0, 2.0),
            Align2::RIGHT_BOTTOM,
            "in",
            FontId::proportional(9.0),
            TEXT_DIM,
        );
        painter.text(
            pos2(to_x(THRESHOLD_DB) + 3.0, plot.bottom() - 2.0),
            Align2::LEFT_BOTTOM,
            "-50 dB",
            FontId::proportional(9.0),
            ACCENT.gamma_multiply(0.8),
        );
    }

    fn draw_activity(&self, painter: &egui::Painter, rect: Rect) {
        let plot = rect.shrink(6.0);
        for d in (-60..=0).step_by(12) {
            let y = plot.bottom() - (d as f32 - DB_MIN) / (DB_MAX - DB_MIN) * plot.height();
            painter.hline(plot.x_range(), y, Stroke::new(1.0_f32, GRID));
        }
        let n = self.history.len();
        if n < 2 {
            return;
        }
        let step = plot.width() / (HISTORY_LEN - 1) as f32;
        let x0 = plot.right() - step * (n - 1) as f32;
        // Detector level as a filled grey area.
        for (i, &(env, _)) in self.history.iter().enumerate() {
            let x = x0 + step * i as f32;
            let y = plot.bottom() - (env - DB_MIN) / (DB_MAX - DB_MIN) * plot.height();
            painter.vline(
                x,
                y..=plot.bottom(),
                Stroke::new(step.max(1.0), Color32::from_gray(70)),
            );
        }
        // Gain reduction hanging from the top.
        let gr: Vec<Pos2> = self
            .history
            .iter()
            .enumerate()
            .map(|(i, &(_, g))| {
                pos2(
                    x0 + step * i as f32,
                    plot.top() + (g / GR_RANGE).min(1.0) * plot.height(),
                )
            })
            .collect();
        painter.line(gr, Stroke::new(1.5_f32, ACCENT));
        painter.text(
            plot.left_top(),
            Align2::LEFT_TOP,
            "GR",
            FontId::proportional(9.0),
            ACCENT,
        );
        painter.text(
            plot.left_bottom(),
            Align2::LEFT_BOTTOM,
            "level",
            FontId::proportional(9.0),
            TEXT_DIM,
        );
    }

    fn draw_meters(&self, painter: &egui::Painter, rect: Rect) {
        let label_h = 14.0;
        let bars = Rect::from_min_max(
            pos2(rect.left(), rect.top() + label_h),
            pos2(rect.right(), rect.bottom() - label_h),
        );
        let bar_w = 7.0;

        // GR pair (top-down).
        let gr_x = rect.left() + 4.0;
        painter.text(
            pos2(gr_x + bar_w + 1.0, rect.top()),
            Align2::CENTER_TOP,
            "GR",
            FontId::proportional(9.0),
            TEXT_DIM,
        );
        for ch in 0..2 {
            let r = Rect::from_min_size(
                pos2(gr_x + ch as f32 * (bar_w + 2.0), bars.top()),
                vec2(bar_w, bars.height()),
            );
            painter.rect_filled(r, 0.0, DISPLAY);
            let h = (self.gr_db[ch] / GR_RANGE).clamp(0.0, 1.0) * r.height();
            painter.rect_filled(Rect::from_min_size(r.min, vec2(bar_w, h)), 0.0, ACCENT);
        }
        let gr = self.gr_db[0].max(self.gr_db[1]);
        painter.text(
            pos2(gr_x + bar_w + 1.0, rect.bottom()),
            Align2::CENTER_BOTTOM,
            format!("{gr:.1}"),
            FontId::proportional(9.0),
            TEXT,
        );

        // Output pair (bottom-up, -60..+0 dBFS with Live-style colour zones).
        let out_x = rect.right() - 2.0 * bar_w - 18.0;
        painter.text(
            pos2(out_x + bar_w + 1.0, rect.top()),
            Align2::CENTER_TOP,
            "Out",
            FontId::proportional(9.0),
            TEXT_DIM,
        );
        for ch in 0..2 {
            let r = Rect::from_min_size(
                pos2(out_x + ch as f32 * (bar_w + 2.0), bars.top()),
                vec2(bar_w, bars.height()),
            );
            painter.rect_filled(r, 0.0, DISPLAY);
            let level = ((self.out_peak[ch] + 60.0) / 60.0).clamp(0.0, 1.0);
            let segments = [
                (0.0, 0.8, METER_LOW),
                (0.8, 0.95, METER_MID),
                (0.95, 1.0, METER_HIGH),
            ];
            for (lo, hi, colour) in segments {
                if level > lo {
                    let top = r.bottom() - level.min(hi) * r.height();
                    let bottom = r.bottom() - lo * r.height();
                    painter.rect_filled(
                        Rect::from_min_max(pos2(r.left(), top), pos2(r.right(), bottom)),
                        0.0,
                        colour,
                    );
                }
            }
        }
        // dB scale between the pairs.
        for d in [0i32, -12, -24, -48] {
            let y = bars.bottom() - (d as f32 + 60.0) / 60.0 * bars.height();
            painter.text(
                pos2(out_x - 3.0, y),
                Align2::RIGHT_CENTER,
                format!("{}", d.abs()),
                FontId::proportional(8.0),
                TEXT_DIM,
            );
        }
        let peak = self.out_peak[0].max(self.out_peak[1]);
        let peak_text = if peak <= DB_MIN {
            "-inf".to_owned()
        } else {
            format!("{peak:.1}")
        };
        painter.text(
            pos2(out_x + bar_w + 1.0, rect.bottom()),
            Align2::CENTER_BOTTOM,
            peak_text,
            FontId::proportional(9.0),
            if peak > -0.1 { METER_HIGH } else { TEXT },
        );
    }
}

fn param_default_normalized(cx: &PluginContext<GersicPressureParams>, id: u32) -> Option<f64> {
    cx.params()
        .param_infos()
        .into_iter()
        .find(|i| i.id == id)
        .map(|i| i.range.normalize(i.default_plain))
}

/// Ableton-style dial: label on top, thin 270° arc with a needle, value below.
/// Drag vertically (Shift for fine), double-click to reset.
fn dial(
    ui: &mut egui::Ui,
    cx: &PluginContext<GersicPressureParams>,
    rect: Rect,
    id: impl Into<u32>,
    label: &str,
    radius: f32,
) {
    let id: u32 = id.into();
    let centre = pos2(rect.center().x, rect.top() + 16.0 + radius + 2.0);
    let hit = Rect::from_center_size(centre, Vec2::splat(radius * 2.0 + 10.0));
    let resp = ui.interact(hit, ui.id().with(("dial", id)), Sense::click_and_drag());

    let mut value = cx.get_param(id);
    if resp.drag_started() {
        cx.begin_edit(id);
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let delta = -resp.drag_delta().y / if fine { 2000.0 } else { 200.0 };
        if delta != 0.0 {
            value = (value + delta).clamp(0.0, 1.0);
            cx.set_param(id, f64::from(value));
        }
    }
    if resp.drag_stopped() {
        cx.end_edit(id);
    }
    if resp.double_clicked()
        && let Some(d) = param_default_normalized(cx, id)
    {
        cx.begin_edit(id);
        cx.set_param(id, d);
        cx.end_edit(id);
        value = d as f32;
    }

    let painter = ui.painter();
    let active = resp.hovered() || resp.dragged();
    painter.text(
        pos2(rect.center().x, rect.top()),
        Align2::CENTER_TOP,
        label,
        FontId::proportional(11.0),
        if active { TEXT } else { TEXT_DIM },
    );

    const START: f32 = std::f32::consts::PI * 0.75;
    const SWEEP: f32 = std::f32::consts::PI * 1.5;
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let n = ((to - from).abs() * 24.0).ceil().max(2.0) as usize;
        (0..=n)
            .map(|i| {
                let a = START + SWEEP * (from + (to - from) * i as f32 / n as f32);
                centre + vec2(a.cos(), a.sin()) * radius
            })
            .collect()
    };
    let width: f32 = if radius > 20.0 { 3.0 } else { 2.5 };
    painter.line(arc(0.0, 1.0), Stroke::new(width, TRACK));
    if value > 0.001 {
        painter.line(arc(0.0, value), Stroke::new(width, ACCENT));
    }
    let a = START + SWEEP * value;
    let dir = vec2(a.cos(), a.sin());
    painter.line_segment(
        [centre + dir * radius * 0.25, centre + dir * radius],
        Stroke::new(2.0_f32, if active { Color32::WHITE } else { TEXT }),
    );

    let text = if id == u32::from(P::ScHpf) && cx.get_param_plain(id) <= SC_HPF_OFF_HZ {
        "Off".to_owned()
    } else {
        cx.format_param(id)
    };
    painter.text(
        pos2(rect.center().x, centre.y + radius + 5.0),
        Align2::CENTER_TOP,
        text,
        FontId::proportional(if radius > 20.0 { 12.0 } else { 10.0 }),
        TEXT,
    );
}

/// Ableton-style rectangular toggle button.
fn toggle(
    ui: &mut egui::Ui,
    cx: &PluginContext<GersicPressureParams>,
    rect: Rect,
    id: impl Into<u32>,
    label: &str,
    tip: &str,
) {
    let id: u32 = id.into();
    let resp = ui
        .interact(rect, ui.id().with(("toggle", id)), Sense::click())
        .on_hover_text(tip);
    let on = cx.get_param(id) > 0.5;
    if resp.clicked() {
        cx.begin_edit(id);
        cx.set_param(id, if on { 0.0 } else { 1.0 });
        cx.end_edit(id);
    }
    let on = cx.get_param(id) > 0.5;
    let painter = ui.painter();
    painter.rect_filled(rect, 1.0, if on { ACCENT } else { HEADER });
    painter.rect_stroke(
        rect,
        1.0,
        Stroke::new(1.0_f32, if resp.hovered() { TEXT_DIM } else { TRACK }),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        if on { Color32::BLACK } else { TEXT_DIM },
    );
}
