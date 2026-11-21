//! Client-side wall time, not GPU timing or whole-process CPU utilization.
pub(crate) mod metrics;
use super::radar::origin;
use super::settings::{Detail, Value};
use super::{key_or_warn, Module, Tick};
use crate::config;
use hudhook::imgui::Ui;
use metrics::{Row, Snapshot};

pub struct PerformanceProfiler {
    enabled: bool,
    key: u16,
    x: i32,
    y: i32,
    rows: usize,
    interval: u64,
    last: Option<u64>,
    paused: bool,
    sort: usize,
    cached: std::sync::Arc<Snapshot>,
    ordered: Vec<usize>,
}
fn order(snapshot: &Snapshot, mode: usize) -> Vec<usize> {
    let mut indices: Vec<_> = (0..snapshot.rows.len()).collect();
    indices.sort_by(|&a, &b| {
        let a = &snapshot.rows[a];
        let b = &snapshot.rows[b];
        let by_name = || {
            a.label
                .name
                .cmp(&b.label.name)
                .then(a.label.phase.cmp(b.label.phase))
        };
        match mode {
            1 => b.max_ns.cmp(&a.max_ns).then_with(by_name),
            2 => b.average_ms().total_cmp(&a.average_ms()).then_with(by_name),
            3 => by_name(),
            _ => b.total_ns.cmp(&a.total_ns).then_with(by_name),
        }
    });
    indices
}
fn panel_layout(display: [f32; 2], line: f32, font: f32, rows: usize) -> ([f32; 2], usize) {
    let fit = (((display[1] - 32.).max(0.) / line) as usize).saturating_sub(4);
    let count = rows.min(fit);
    (
        [
            (760. * (font / 16.).clamp(0.75, 1.75)).min(display[0]),
            (32. + line * (count.max(1) + 4) as f32).min(display[1]),
        ],
        count,
    )
}
impl PerformanceProfiler {
    pub fn new(c: &config::PerformanceProfiler) -> Self {
        Self {
            enabled: false,
            key: key_or_warn("performanceprofiler", &c.key),
            x: c.x.clamp(-7680, 7680),
            y: c.y.clamp(-4320, 4320),
            rows: c.rows.clamp(5, 24),
            interval: c.update_ms.clamp(200, 5000),
            last: None,
            paused: false,
            sort: 0,
            cached: Default::default(),
            ordered: Vec::new(),
        }
    }
    fn reset(&mut self) {
        metrics::start();
        self.last = None;
        self.paused = false;
        self.cached = Default::default();
        self.ordered.clear();
    }
    fn row_name(row: &Row) -> String {
        format!(
            "{} / {}{}",
            row.label.name,
            row.label.phase,
            if row.label.inclusive { " *" } else { "" }
        )
    }
}
impl Module for PerformanceProfiler {
    fn name(&self) -> &str {
        "performanceprofiler"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if on {
            self.reset();
        } else {
            metrics::stop();
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::integer(
                "rows",
                "表示行数",
                "重い処理から表示します。詳細画面では全件を確認できます。",
                self.rows as i32,
                5,
                24,
            ),
            Detail::integer(
                "update_ms",
                "集計間隔（ms）",
                "平均・最大・回数は直近の集計区間の値です。",
                self.interval as i32,
                200,
                5000,
            ),
            Detail::x_offset(self.x, "Negative: from right."),
            Detail::y_offset(self.y, "Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        let n = value.number()?;
        if !n.is_finite() {
            return Err("有限の数値を指定してください".into());
        }
        match id {
            "rows" => self.rows = (n as usize).clamp(5, 24),
            "update_ms" => {
                self.interval = (n as u64).clamp(200, 5000);
            }
            "x" => self.x = (n as i32).clamp(-7680, 7680),
            "y" => self.y = (n as i32).clamp(-4320, 4320),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn tick(&mut self, tick: &Tick) {
        if !self.enabled || self.paused {
            return;
        }
        let Some(last) = self.last else {
            self.last = Some(tick.now_ms);
            return;
        };
        let elapsed = tick.now_ms.saturating_sub(last);
        if elapsed >= self.interval {
            metrics::refresh(elapsed as f64 / 1000.);
            self.cached = metrics::snapshot();
            self.ordered = order(&self.cached, self.sort);
            self.last = Some(tick.now_ms);
        }
    }
    fn status(&self) -> String {
        if self.paused {
            "計測停止中".into()
        } else if self.cached.rows.is_empty() {
            "計測中・集計待ち".into()
        } else {
            format!(
                "{}経路 / 集計 {:.0} ms",
                self.cached.rows.len(),
                self.cached.seconds * 1000.
            )
        }
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.text_wrapped("クライアントの各処理にかかった経過時間です。GPU時間・ゲーム本体の全負荷・CPU使用率は含みません。待機や計測自体の影響を含みます。");
        ui.text_wrapped("* は子処理を含む全体時間です。行同士を足し合わせないでください。件/秒は読取ブロック数・走査列数・出力エンティティ数を各行の単位で示します。");
        if ui.combo_simple_string(
            "並び順",
            &mut self.sort,
            &["合計時間", "最大時間", "平均時間", "名前"],
        ) {
            self.ordered = order(&self.cached, self.sort);
        }
        let _disabled = ui.begin_disabled(!self.enabled);
        if ui.button(if self.paused {
            "計測を再開"
        } else {
            "計測を停止"
        }) {
            if self.paused {
                self.reset();
            } else {
                metrics::stop();
                self.paused = true;
            }
        }
        ui.same_line();
        if ui.button("計測をリセット") {
            self.reset();
        }
        drop(_disabled);
        if ui.button("計測結果をコピー") {
            ui.set_clipboard_text(report(&self.cached));
        }
        ui.text_wrapped(format!(
            "直近 {:.3} 秒 / 競合による欠測 {}件 / 登録上限による除外 {}経路",
            self.cached.seconds, self.cached.dropped, self.cached.omitted
        ));
        ui.columns(6, "profiler_columns", false);
        for title in ["処理", "平均ms", "最大ms", "ms/秒", "回/秒", "件/秒"] {
            ui.text(title);
            ui.next_column();
        }
        ui.separator();
        for &index in &self.ordered {
            let row = &self.cached.rows[index];
            for value in [
                Self::row_name(row),
                format!("{:.4}", row.average_ms()),
                format!("{:.4}", row.max_ns as f64 / 1e6),
                format!("{:.3}", row.total_ns as f64 / 1e6 / self.cached.seconds),
                format!("{:.1}", row.calls as f64 / self.cached.seconds),
                format!("{:.1}", row.units as f64 / self.cached.seconds),
            ] {
                ui.text(value);
                ui.next_column();
            }
        }
        ui.columns(1, "", false);
    }
    fn draw(&mut self, ui: &Ui) {
        let display = ui.io().display_size;
        let line = ui.text_line_height_with_spacing();
        let (size, count) = panel_layout(
            display,
            line,
            ui.current_font_size(),
            self.rows.min(self.ordered.len()),
        );
        crate::hud_editor::measure("performanceprofiler", size);
        let left = origin(self.x, display[0], size[0]);
        let top = origin(self.y, display[1], size[1]);
        let draw = ui.get_background_draw_list();
        crate::theme::panel(&draw, [left, top], [left + size[0], top + size[1]]);
        draw.with_clip_rect([left, top], [left + size[0], top + size[1]], || {
            draw.add_text(
                [left + 12., top + 9.],
                crate::theme::TEXT,
                format!(
                    "PerformanceProfiler  / {:.0} ms{}",
                    self.cached.seconds * 1000.,
                    if self.paused { " / 停止中" } else { "" }
                ),
            );
            let columns = [12., 320., 405., 490., 575., 660.].map(|x| x * size[0] / 760.);
            for (i, title) in ["処理 / 区分", "平均ms", "最大ms", "ms/秒", "回/秒", "件/秒"]
                .iter()
                .enumerate()
            {
                draw.add_text(
                    [left + columns[i], top + 12. + line],
                    crate::theme::MUTED,
                    title,
                );
            }
            for (shown, &index) in self.ordered.iter().take(count).enumerate() {
                let row = &self.cached.rows[index];
                let y = top + 14. + line * (shown + 2) as f32;
                let name = Self::row_name(row);
                draw.with_clip_rect_intersect(
                    [left + 12., y],
                    [left + columns[1] - 10., y + line],
                    || {
                        draw.add_text([left + 12., y], crate::theme::TEXT, name);
                    },
                );
                let values = [
                    format!("{:.3}", row.average_ms()),
                    format!("{:.3}", row.max_ns as f64 / 1e6),
                    format!("{:.2}", row.total_ns as f64 / 1e6 / self.cached.seconds),
                    format!("{:.0}", row.calls as f64 / self.cached.seconds),
                    format!("{:.0}", row.units as f64 / self.cached.seconds),
                ];
                for (i, v) in values.iter().enumerate() {
                    let end = columns.get(i + 2).copied().unwrap_or(size[0]) - 8.;
                    draw.with_clip_rect_intersect(
                        [left + columns[i + 1], y],
                        [left + end, y + line],
                        || {
                            draw.add_text([left + columns[i + 1], y], crate::theme::TEXT, v);
                        },
                    );
                }
            }
            if self.ordered.is_empty() {
                draw.add_text(
                    [left + 12., top + 14. + line * 2.],
                    crate::theme::TEXT,
                    "計測中・集計待ち",
                );
            }
            draw.add_text(
                [left + 12., top + size[1] - line * 2. - 8.],
                crate::theme::MUTED,
                "* は子処理を含みます。GPU時間・全体CPU使用率ではありません。",
            );
            draw.add_text(
                [left + 12., top + size[1] - line - 6.],
                crate::theme::MUTED,
                format!(
                    "競合欠測 {} / 上限除外 {}  | 詳細で停止・リセット・コピー",
                    self.cached.dropped, self.cached.omitted
                ),
            );
        });
    }
}
fn report(snapshot: &Snapshot) -> String {
    use std::fmt::Write;
    let mut text = format!("PerformanceProfiler: active={} window={:.3}s dropped={} omitted={}\nCPU経路の経過時間。GPU時間・CPU使用率ではありません。* は子処理を含むため加算不可。\n処理\t区分\t平均ms\t最大ms\tms/秒\t回/秒\t件/秒\n",
        metrics::active(), snapshot.seconds, snapshot.dropped, snapshot.omitted);
    let seconds = snapshot.seconds.max(0.001);
    for i in order(snapshot, 0) {
        let row = &snapshot.rows[i];
        let _ = writeln!(
            text,
            "{}{}\t{}\t{:.6}\t{:.6}\t{:.3}\t{:.2}\t{:.2}",
            row.label.name,
            if row.label.inclusive { " *" } else { "" },
            row.label.phase,
            row.average_ms(),
            row.max_ns as f64 / 1e6,
            row.total_ns as f64 / 1e6 / seconds,
            row.calls as f64 / seconds,
            row.units as f64 / seconds
        );
    }
    text
}
pub(crate) fn diagnostic() -> String {
    report(&metrics::snapshot())
}

#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    let mut panel = PerformanceProfiler::new(&config::PerformanceProfiler::default());
    panel.cached = std::sync::Arc::new(Snapshot {
        rows: (0..30)
            .map(|i| Row {
                label: metrics::Label {
                    name: format!("Feature {i}"),
                    phase: "描画",
                    inclusive: i == 0,
                },
                calls: 100,
                total_ns: 1_000_000,
                max_ns: 100_000,
                units: 1234,
            })
            .collect(),
        seconds: 1.,
        ..Default::default()
    });
    panel.ordered = order(&panel.cached, 0);
    panel.rows = 24;
    panel.draw(ui);
    ui.window("PerformanceProfiler details")
        .size([700., 650.], hudhook::imgui::Condition::Always)
        .build(|| panel.draw_settings(ui));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enlarged_fonts_keep_footer_below_rows_and_panel_inside_screen() {
        for (display, font) in [
            ([1280., 720.], 12.),
            ([1280., 720.], 28.),
            ([2560., 1440.], 16.),
        ] {
            let line = font + 4.;
            let (size, rows) = panel_layout(display, line, font, 24);
            assert!(size[0] <= display[0] && size[1] <= display[1]);
            let row_bottom = 14. + line * (rows + 2) as f32;
            let footer = size[1] - line * 2. - 8.;
            assert!(footer >= row_bottom, "font={font} rows={rows}");
        }
    }
    #[test]
    fn sorting_distinguishes_per_call_spikes_from_total_cost() {
        let row = |name: &str, calls, ns, peak| Row {
            label: metrics::Label {
                name: name.into(),
                phase: "更新",
                inclusive: false,
            },
            calls,
            total_ns: ns,
            max_ns: peak,
            units: 0,
        };
        let s = Snapshot {
            rows: vec![
                row("frequent", 100, 100_000, 1000),
                row("spike", 1, 50_000, 50_000),
            ],
            seconds: 1.,
            ..Default::default()
        };
        assert_eq!(order(&s, 0), [0, 1]);
        assert_eq!(order(&s, 1), [1, 0]);
        assert_eq!(order(&s, 2), [1, 0]);
        let text = report(&s);
        assert!(text.contains("0.100"));
        assert!(!text.contains("NaN"));
        assert!(!report(&Snapshot::default()).contains("NaN"));
    }
}
