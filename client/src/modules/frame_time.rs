//! FrameTime: a graph of the last few seconds of frame times, so stutter
//! (single long frames) shows even when the average frame rate looks fine.
//! Measured from the overlay's own frames, which are the game's frames.

use std::collections::VecDeque;

use hudhook::imgui::Ui;

use super::radar::origin;
use super::settings::{Detail, Value};
use super::{key_or_warn, Module};
use crate::config;

/// How much history the graph shows (seconds).
const WINDOW: f32 = 5.;
const WIDTH: f32 = 280.;
const HEIGHT: f32 = 110.;
const PADDING: f32 = 8.;

const GOOD: [f32; 4] = [0.43, 0.85, 0.71, 1.];
const SLOW: [f32; 4] = [0.98, 0.75, 0.41, 1.];
const BAD: [f32; 4] = [0.97, 0.45, 0.49, 1.];
const GUIDE: [f32; 4] = [0.72, 0.82, 0.95, 0.18];
const MAX_FRAMES: usize = 8192;

/// Frame times (ms), newest last, covering about `WINDOW` seconds.
#[derive(Default)]
struct History {
    frames: VecDeque<f32>,
    total: f32,
}

impl History {
    fn push(&mut self, ms: f32) {
        if !(ms.is_finite() && ms > 0.) {
            return;
        }
        // Long pauses (a loading screen) would flatten the graph for seconds.
        let ms = ms.min(1000.);
        self.frames.push_back(ms);
        self.total += ms;
        while (self.total > WINDOW * 1000. || self.frames.len() > MAX_FRAMES)
            && self.frames.len() > 1
        {
            self.total -= self.frames.pop_front().unwrap_or(0.);
        }
    }

    /// (average ms, longest ms, "1% low" fps: the frame rate of the slowest
    /// 1% of frames).
    fn summary(&self) -> Option<(f32, f32, f32)> {
        if self.frames.is_empty() {
            return None;
        }
        let average = self.total / self.frames.len() as f32;
        let mut sorted: Vec<f32> = self.frames.iter().copied().collect();
        sorted.sort_by(|a, b| b.total_cmp(a));
        let slowest = (sorted.len() / 100).max(1);
        let low = sorted[..slowest].iter().sum::<f32>() / slowest as f32;
        Some((average, sorted[0], 1000. / low))
    }
}

/// Green within 60 fps, amber within 30 fps, red beyond.
fn color(ms: f32) -> [f32; 4] {
    if ms <= 1000. / 60. + 0.5 {
        GOOD
    } else if ms <= 1000. / 30. + 0.5 {
        SLOW
    } else {
        BAD
    }
}

pub struct FrameTime {
    key: u16,
    enabled: bool,
    x: i32,
    y: i32,
    history: History,
    line: bool,
    cached: Option<(f32, f32, f32)>,
    summary_at: std::time::Instant,
    /// Reused per frame: the decimated points or bars.
    points: Vec<([f32; 2], f32)>,
}

impl FrameTime {
    pub fn new(cfg: &config::FrameTime) -> Self {
        Self {
            key: key_or_warn("frametime", &cfg.key),
            enabled: false,
            x: cfg.x,
            y: cfg.y,
            history: History::default(),
            line: cfg.line,
            cached: None,
            summary_at: std::time::Instant::now(),
            points: Vec::new(),
        }
    }
}

impl Module for FrameTime {
    fn name(&self) -> &str {
        "frametime"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        self.history = History::default();
        self.cached = None;
        self.enabled = on;
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "line",
                "折れ線で表示",
                "OFFにすると従来の棒グラフへ戻します。",
                self.line,
            ),
            Detail::x_offset(self.x, "Positive: from left. Negative: from right."),
            Detail::y_offset(self.y, "Positive: from top. Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "x" => self.x = value.number()? as i32,
            "y" => self.y = value.number()? as i32,
            "line" => self.line = value.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        match self.cached {
            Some((average, longest, _)) => format!("平均 {average:.1} ms / 最大 {longest:.1} ms"),
            None => String::new(),
        }
    }
    fn draw(&mut self, ui: &Ui) {
        self.history.push(ui.io().delta_time * 1000.);
        if self.cached.is_none() || self.summary_at.elapsed().as_millis() >= 250 {
            self.cached = self.history.summary();
            self.summary_at = std::time::Instant::now();
        }
        let screen = ui.io().display_size;
        let size = [WIDTH.min(screen[0]), HEIGHT.min(screen[1])];
        crate::hud_editor::measure("frametime", size);
        let left = origin(self.x, screen[0], size[0]);
        let top = origin(self.y, screen[1], size[1]);
        let draw = ui.get_background_draw_list();
        crate::theme::panel(&draw, [left, top], [left + size[0], top + size[1]]);
        let Some((average, longest, low)) = self.cached else {
            return;
        };
        let line = ui.text_line_height();
        draw.add_text(
            [left + PADDING, top + PADDING - 2.],
            crate::theme::TEXT,
            format!("{average:.1} ms  ({:.0} fps)", 1000. / average),
        );
        draw.add_text(
            [left + PADDING, top + PADDING - 2. + line],
            crate::theme::MUTED,
            format!("最大 {longest:.1} ms   1% low {low:.0} fps"),
        );
        // The graph: newest frame at the right edge, one column per frame,
        // scaled so 33 ms (30 fps) or the longest frame fits.
        let graph = [
            left + PADDING,
            top + PADDING + 2. * line + 2.,
            left + size[0] - PADDING,
            top + size[1] - PADDING,
        ];
        let height = (graph[3] - graph[1]).max(1.);
        let scale = longest.max(1000. / 30. + 2.);
        let y = |ms: f32| graph[3] - (ms / scale).min(1.) * height;
        for guide in [1000. / 60., 1000. / 30.] {
            draw.add_line([graph[0], y(guide)], [graph[2], y(guide)], GUIDE)
                .build();
        }
        // One point (line) or bar per frame, newest at the right edge. At
        // high frame rates several frames share a pixel column; `decimate`
        // keeps only what can show, so the cost no longer grows with fps.
        let span = WINDOW * 1000.;
        let width = graph[2] - graph[0];
        let mut x = graph[2];
        let frames = self.history.frames.iter().rev().map_while(|&ms| {
            if x <= graph[0] {
                return None;
            }
            let next = (x - (ms / span) * width).max(graph[0]);
            let bar = (next, x, ms);
            x = next;
            Some(bar)
        });
        if self.line {
            let points = frames
                .map(|(next, x, ms)| ([((x + next) * 0.5).clamp(graph[0], graph[2]), y(ms)], ms));
            decimate(points, &mut self.points);
            stroke(&draw, &self.points);
        } else {
            bars(frames, graph[2], &mut self.points);
            for &([left, right], ms) in &self.points {
                draw.add_rect([left, y(ms)], [right, graph[3]], color(ms))
                    .filled(true)
                    .build();
            }
        }
    }
}

/// Min/max decimation of a line given newest first as (point, ms): within
/// each pixel column only its highest and lowest points are kept, in the
/// order they occur. The dropped segments lay inside that column's band, so
/// the band and every spike look the same with at most two points a column.
fn decimate(points: impl Iterator<Item = ([f32; 2], f32)>, out: &mut Vec<([f32; 2], f32)>) {
    out.clear();
    let mut column = None;
    // Topmost (smallest y) and bottommost points of the current column, and
    // whether the topmost came first.
    let (mut high, mut low, mut high_first) = (([0.; 2], 0.), ([0.; 2], 0.), true);
    let flush = |out: &mut Vec<_>, high: ([f32; 2], f32), low: ([f32; 2], f32), high_first| {
        let (first, second) = if high_first { (high, low) } else { (low, high) };
        out.push(first);
        if second.0 != first.0 {
            out.push(second);
        }
    };
    for point in points {
        let here = point.0[0].floor() as i32;
        if column != Some(here) {
            if column.is_some() {
                flush(out, high, low, high_first);
            }
            column = Some(here);
            (high, low, high_first) = (point, point, true);
            continue;
        }
        if point.0[1] < high.0[1] {
            high = point;
            high_first = false;
        }
        if point.0[1] > low.0[1] {
            low = point;
            // Whichever extreme was updated last comes second.
            high_first = true;
        }
    }
    if column.is_some() {
        flush(out, high, low, high_first);
    }
}

/// Bars given newest first as (left, right, ms), merged per pixel column of
/// their right edge into the tallest (what overlapping bars show), each at
/// least a pixel wide but not past `limit` (the graph's right edge):
/// ([left, right], ms).
fn bars(frames: impl Iterator<Item = (f32, f32, f32)>, limit: f32, out: &mut Vec<([f32; 2], f32)>) {
    out.clear();
    let mut column = None;
    for (left, right, ms) in frames {
        let here = right.floor() as i32;
        match out.last_mut() {
            Some(([l, _], m)) if column == Some(here) => {
                *l = l.min(left);
                *m = m.max(ms);
            }
            _ => {
                column = Some(here);
                out.push(([left, right.max(left + 1.).min(limit)], ms));
            }
        }
    }
}

/// Strokes the points newest first, a line per segment in the colour of its
/// older point, as before. Not one polyline per colour: the decimated band
/// turns sharply within a pixel, and ImGui's mitred joins would poke out of
/// it by several pixels.
fn stroke(draw: &hudhook::imgui::DrawListMut<'_>, points: &[([f32; 2], f32)]) {
    for pair in points.windows(2) {
        let (newer, (older, ms)) = (pair[0].0, pair[1]);
        draw.add_line(older, newer, color(ms))
            .thickness(1.5)
            .build();
    }
}

/// Five seconds at about 144 fps with a few hitches, for the UI preview.
#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    let mut graph = FrameTime::new(&config::FrameTime {
        x: -20,
        y: 250,
        ..Default::default()
    });
    for i in 0..700u32 {
        let ms = match i % 173 {
            0 => 48.,
            1..=3 => 22.,
            _ => 6.9 + (i % 7) as f32 * 0.15,
        };
        graph.history.push(ms);
    }
    graph.draw(ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of the panel with a full 5 s history (jittery, with spikes)
    /// at several frame rates, line and bar modes.
    #[test]
    #[ignore = "manual: FrameTime panel cost; MCBE_UI_PREVIEW exports frames"]
    fn frame_time_paint_cost() {
        let mut ctx = hudhook::imgui::Context::create();
        ctx.set_ini_filename(None);
        let texture = ctx.fonts().build_alpha8_texture();
        let atlas = ([texture.width, texture.height], texture.data.to_vec());
        ctx.io_mut().display_size = [1280., 720.];
        for fps in [60., 144., 240., 500.] {
            for line in [true, false] {
                let mut panel = FrameTime::new(&config::FrameTime {
                    line,
                    x: 10,
                    y: 10,
                    ..Default::default()
                });
                panel.enabled = true;
                let base = 1000. / fps;
                let mut seed = 1u32;
                let mut jitter = || {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1u32 << 24) as f32
                };
                let mut times = Vec::new();
                let mut vertices = 0;
                for frame in 0..(fps as usize * 6) {
                    let spike = if frame % 97 == 0 { 40. } else { 0. };
                    ctx.io_mut().delta_time = (base * (0.7 + 0.6 * jitter()) + spike) / 1000.;
                    let ui = ctx.frame();
                    let start = std::time::Instant::now();
                    panel.draw(ui);
                    let elapsed = start.elapsed();
                    let data = ctx.render();
                    vertices = data.total_vtx_count;
                    if fps == 144. && line && frame + 1 == fps as usize * 6 {
                        if let Some(dir) = std::env::var_os("MCBE_UI_PREVIEW") {
                            crate::gui::preview::export_frame(
                                &std::path::Path::new(&dir).join("frametime"),
                                data,
                                &atlas,
                            );
                        }
                    }
                    if frame >= fps as usize * 5 {
                        times.push(elapsed);
                    }
                }
                times.sort();
                println!(
                    "FrameTime {fps} fps ({}): median {:?}, p90 {:?}, {} frames kept, {vertices} vertices",
                    if line { "line" } else { "bars" },
                    times[times.len() / 2],
                    times[times.len() * 9 / 10],
                    panel.history.frames.len()
                );
            }
        }
    }
    #[test]
    fn decimation_keeps_each_columns_extremes_in_order() {
        let mut out = Vec::new();
        // Column 10: four frames zigzagging; column 9: one frame.
        let points = [
            ([10.9, 50.], 1.),
            ([10.6, 40.], 2.),
            ([10.4, 70.], 3.),
            ([10.1, 55.], 4.),
            ([9.5, 60.], 5.),
        ];
        decimate(points.into_iter(), &mut out);
        // Highest (y 40) then lowest (y 70), as they occurred; then column 9.
        assert_eq!(
            out,
            vec![([10.6, 40.], 2.), ([10.4, 70.], 3.), ([9.5, 60.], 5.)]
        );
        // Lowest first when it came first.
        decimate([([3.2, 80.], 1.), ([3.1, 20.], 2.)].into_iter(), &mut out);
        assert_eq!(out, vec![([3.2, 80.], 1.), ([3.1, 20.], 2.)]);
        // A flat column keeps one point; nothing in, nothing out.
        decimate([([5.5, 30.], 1.), ([5.2, 30.], 1.)].into_iter(), &mut out);
        assert_eq!(out.len(), 1);
        decimate(std::iter::empty(), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn bars_merge_per_column_into_the_tallest() {
        let mut out = Vec::new();
        // Right edges 10.9 and 10.6 share column 10; 9.8 is column 9.
        bars(
            [(10.6, 10.9, 5.), (10.2, 10.6, 30.), (8.5, 9.8, 8.)].into_iter(),
            20.,
            &mut out,
        );
        assert_eq!(out, vec![([10.2, 11.6], 30.), ([8.5, 9.8], 8.)]);
        // A sliver is still a pixel wide, but never past the graph's edge.
        bars([(4.9, 5.0, 1.)].into_iter(), 20., &mut out);
        assert_eq!(out, vec![([4.9, 5.9], 1.)]);
        bars([(19.8, 20.0, 1.)].into_iter(), 20., &mut out);
        assert_eq!(out, vec![([19.8, 20.0], 1.)]);
    }

    #[test]
    fn history_stays_bounded_when_frame_deltas_are_tiny() {
        let mut h = History::default();
        for _ in 0..20000 {
            h.push(0.001);
        }
        assert_eq!(h.frames.len(), MAX_FRAMES);
        assert!(h.summary().unwrap().0.is_finite());
    }

    #[test]
    fn history_keeps_about_five_seconds_and_reports_the_slowest_frames() {
        let mut h = History::default();
        assert!(h.summary().is_none());
        for _ in 0..600 {
            h.push(10.);
        }
        // 5 s of 10 ms frames.
        assert_eq!(h.frames.len(), 500);
        h.push(50.);
        let (average, longest, low) = h.summary().unwrap();
        assert_eq!(longest, 50.);
        assert!((average - 10.).abs() < 0.2);
        // The slowest 1% (4 of 496 frames): the 50 ms frame and three 10 ms.
        assert_eq!(h.frames.len(), 496);
        assert!((low - 50.).abs() < 0.01, "{low}");
        // Pauses are capped and junk is ignored.
        h.push(f32::NAN);
        h.push(-1.);
        h.push(60_000.);
        assert_eq!(h.summary().unwrap().1, 1000.);
    }

    #[test]
    fn colours_follow_60_and_30_fps() {
        assert_eq!(color(16.6), GOOD);
        assert_eq!(color(25.), SLOW);
        assert_eq!(color(40.), BAD);
    }
}
