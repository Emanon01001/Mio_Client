//! Passive feature card. Geometry only: no window, input capture, textures or blur pass.
use crate::localization::module_name;
use hudhook::imgui::Ui;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tone {
    Ready,
    Quiet,
    Warning,
}
impl Tone {
    fn text(self) -> [f32; 4] {
        match self {
            Self::Ready => crate::theme::SUCCESS,
            Self::Quiet => crate::theme::MUTED,
            Self::Warning => crate::theme::WARNING,
        }
    }
    fn fill(self) -> [f32; 4] {
        match self {
            Self::Ready => [0.21, 0.63, 0.49, 0.13],
            Self::Quiet => [0.50, 0.59, 0.70, 0.10],
            Self::Warning => [0.95, 0.62, 0.20, 0.12],
        }
    }
}
struct Status<'a> {
    text: &'a str,
    badge: bool,
    tone: Tone,
}
fn origin(offset: i32, screen: f32, length: f32) -> f32 {
    let value = if offset < 0 {
        screen + offset as f32 - length
    } else {
        offset as f32
    };
    value.clamp(0., (screen - length).max(0.))
}
fn status<'a>(id: &str, source: &'a str) -> Status<'a> {
    let (text, badge, tone) = match source.lines().next().unwrap_or("").trim() {
        "" => ("有効", true, Tone::Ready),
        "（ワールド未接続）" => ("未接続", true, Tone::Quiet),
        "（データなし）" | "（所持品データなし）" => {
            ("データ待ち", true, Tone::Quiet)
        }
        "インベントリ中の移動が有効" if id == "inventorymove" => {
            ("移動可能", true, Tone::Ready)
        }
        "インベントリを開くと有効" if id == "inventorymove" => {
            ("待機中", true, Tone::Quiet)
        }
        other if other.contains("一時停止") => (other, false, Tone::Warning),
        other => (other, false, Tone::Quiet),
    };
    Status { text, badge, tone }
}
/// Card geometry in pixels at the 16 px font (scaled with the font).
#[derive(Clone, Copy)]
struct Metrics {
    width: f32,
    max_width: f32,
    padding: f32,
    header: f32,
    top: f32,
    bottom: f32,
    /// One line. Additional lines use the font height plus a small gap.
    row: f32,
    /// Text inset from the top of a row.
    row_inset: f32,
    mark: f32,
    rounding: f32,
}
const REGULAR: Metrics = Metrics {
    width: 320.,
    max_width: 440.,
    padding: 16.,
    header: 40.,
    top: 8.,
    bottom: 10.,
    row: 30.,
    row_inset: 7.,
    mark: 22.,
    rounding: 12.,
};
const COMPACT: Metrics = Metrics {
    width: 236.,
    max_width: 336.,
    padding: 10.,
    header: 28.,
    top: 3.,
    bottom: 5.,
    row: 23.,
    row_inset: 3.5,
    mark: 16.,
    rounding: 9.,
};
fn metrics(compact: bool, vertical: bool) -> Metrics {
    let mut m = if compact { COMPACT } else { REGULAR };
    if vertical {
        m.row = if compact { 19. } else { 26. };
        m.row_inset = if compact { 1.5 } else { 5. };
    }
    m
}
/// The most columns the list is split into.
const MAX_COLUMNS: usize = 3;
/// In automatic mode the list is kept within this share of the screen height.
const AUTO_HEIGHT: f32 = 0.45;

/// The number of columns for rows of `heights`, filled top to bottom: the
/// `wanted` number (0: automatic, the fewest whose tallest column is within
/// `limit`), never more than `max` (what fits across the screen).
fn columns(heights: &[f32], wanted: usize, limit: f32, max: usize) -> usize {
    let max = max.clamp(1, MAX_COLUMNS);
    if wanted > 0 {
        return wanted.min(max);
    }
    (1..=max)
        .find(|&n| tallest(heights, n) <= limit)
        .unwrap_or(max)
}

/// The height of the tallest of `n` columns of `heights`.
fn tallest(heights: &[f32], n: usize) -> f32 {
    partitions(heights, n)
        .into_iter()
        .map(|c| heights[c].iter().sum::<f32>())
        .fold(0., f32::max)
}

/// Keep source order, balancing pixel height rather than the number of rows.
fn partitions(heights: &[f32], n: usize) -> Vec<Range<usize>> {
    let count = n.max(1).min(heights.len().max(1));
    let mut result = Vec::with_capacity(count);
    let mut start = 0;
    let mut remaining = heights.iter().sum::<f32>();
    for column in 0..count {
        let left = count - column;
        if left == 1 {
            result.push(start..heights.len());
            break;
        }
        let target = remaining / left as f32;
        let mut end = start;
        let mut used = 0.;
        let limit = heights.len() - (left - 1);
        while end < limit {
            let next = used + heights[end];
            if end > start && (next - target).abs() > (used - target).abs() {
                break;
            }
            used = next;
            end += 1;
        }
        result.push(start..end);
        start = end;
        remaining -= used;
    }
    result
}

/// Borrowed UTF-8 ranges: no ellipsis, dropped characters or temporary strings.
fn wrap(text: &str, available: f32, measure: impl Fn(&str) -> f32) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let rest = &text[start..];
        if measure(rest) <= available {
            ranges.push(start..text.len());
            break;
        }
        let (mut low, mut high) = (0, rest.len());
        while low < high {
            let mut end = low + (high - low).div_ceil(2);
            while !rest.is_char_boundary(end) {
                end += 1;
            }
            if measure(&rest[..end]) <= available {
                low = end;
            } else {
                high = end - 1;
                while !rest.is_char_boundary(high) {
                    high -= 1;
                }
            }
        }
        // If even one glyph cannot fit, advance anyway and clip to this column.
        let mut end = low.max(rest.chars().next().unwrap().len_utf8());
        if let Some((at, ch)) = rest[..end]
            .char_indices()
            .rev()
            .find(|(at, ch)| *at >= end / 2 && ch.is_whitespace())
        {
            end = at + ch.len_utf8();
        }
        ranges.push(start..start + end);
        start += end;
    }
    if ranges.is_empty() {
        ranges.push(0..0);
    }
    ranges
}

#[derive(Default)]
struct Row {
    id: String,
    name: String,
    value: String,
    badge: bool,
    tone: Option<Tone>,
    name_width: f32,
    value_width: f32,
    plan: Option<RowPlan>,
}
struct RowPlan {
    content: f32,
    text_scale: f32,
    names: Vec<Range<usize>>,
    values: Vec<Range<usize>>,
    stacked: bool,
    height: f32,
}
impl Row {
    fn update(
        &mut self,
        id: &str,
        state: Status<'_>,
        font_changed: bool,
        scale: f32,
        measure: impl Fn(&str) -> f32,
    ) -> bool {
        let renamed = self.id != id;
        if renamed {
            self.id.clear();
            self.id.push_str(id);
            self.name.clear();
            self.name
                .push_str(module_name(id).lines().next().unwrap_or(""));
        }
        let changed = renamed
            || self.value != state.text
            || self.badge != state.badge
            || font_changed
            || self.tone.is_none();
        if renamed || font_changed {
            self.name_width = measure(&self.name);
        }
        if changed {
            self.value.clear();
            self.value.push_str(state.text);
            self.value_width = measure(&self.value) + if state.badge { 16. * scale } else { 0. };
            self.plan = None;
        }
        self.badge = state.badge;
        self.tone = Some(state.tone);
        changed
    }
    fn plan(
        &mut self,
        content: f32,
        m: &Metrics,
        font: f32,
        scale: f32,
        text_scale: f32,
        measure: impl Fn(&str) -> f32,
    ) {
        if self
            .plan
            .as_ref()
            .is_some_and(|p| p.content == content && p.text_scale == text_scale)
        {
            return;
        }
        let stacked = (self.name_width + self.value_width) * text_scale + 18. * scale > content;
        let (names, values) = if stacked {
            (
                wrap(&self.name, content, &measure),
                wrap(
                    &self.value,
                    (content - if self.badge { 16. * scale } else { 0. }).max(1.),
                    &measure,
                ),
            )
        } else {
            (
                std::iter::once(0..self.name.len()).collect(),
                std::iter::once(0..self.value.len()).collect(),
            )
        };
        let lines = if stacked {
            names.len() + values.len()
        } else {
            1
        };
        self.plan = Some(RowPlan {
            content,
            text_scale,
            names,
            values,
            stacked,
            height: m.row * scale + (lines - 1) as f32 * (font + 4. * scale),
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    screen: [f32; 2],
    font: f32,
    font_id: usize,
    compact: bool,
    columns: usize,
    column_width: f32,
}
/// One drawn line of the list: a single row, or (vertical list) two short
/// rows side by side.
struct Line {
    rows: Range<usize>,
    height: f32,
}

/// Groups rows into lines in their order: two neighbouring `short` rows share
/// a line, anything else gets a line of its own.
fn pack(short: &[bool]) -> Vec<Range<usize>> {
    let mut lines = Vec::with_capacity(short.len());
    let mut i = 0;
    while i < short.len() {
        let n = if short[i] && short.get(i + 1) == Some(&true) {
            2
        } else {
            1
        };
        lines.push(i..i + n);
        i += n;
    }
    lines
}

/// Width of one cell of a line holding `rows` rows (the dot, name and value
/// all sit inside it); two cells are separated by `padding`.
fn cell_width(column: f32, padding: f32, rows: usize) -> f32 {
    if rows > 1 {
        ((column - padding * 3.) * 0.5).max(1.)
    } else {
        (column - padding * 2.).max(1.)
    }
}

struct Layout {
    column: f32,
    width: f32,
    height: f32,
    lines: Vec<Line>,
    /// Ranges of `lines`, one per column.
    chunks: Vec<Range<usize>>,
    /// Lines shown in each column.
    shown: Vec<usize>,
    hidden: usize,
    required_height: f32,
    fit_scale: f32,
    vertical: bool,
    viewport: f32,
}
impl Layout {
    fn new(rows: &mut [Row], g: Geometry, ui: &Ui, fit_scale: f32) -> Self {
        let m = metrics(g.compact, g.columns == 1);
        let font = g.font * fit_scale;
        let scale = font / 16.;
        let padding = m.padding * scale;
        let overhead = m.header * scale + m.top * scale + m.bottom * scale;
        let one = m.row * scale;
        let limit = (g.screen[1] * AUTO_HEIGHT - overhead).max(one);
        let available = (g.screen[1] - 8. - overhead).max(0.);
        let base_column = (m.width * scale).min(g.screen[0] - 8.);
        // Allow narrower columns at large fonts when full names still fit.
        let widest_name = rows
            .iter()
            .map(|r| r.name_width * fit_scale)
            .fold(0., f32::max);
        let min_column = (widest_name + padding * 2. + 14. * scale)
            .max(if g.compact { 180. } else { 240. } * scale)
            .min(base_column);
        let max = (((g.screen[0] - 8. + padding) / (min_column + padding)).floor() as usize)
            .clamp(1, MAX_COLUMNS)
            .min(rows.len().max(1));
        let desired = rows
            .iter()
            .map(|r| {
                (r.name_width + r.value_width) * fit_scale + (18. + 14. + m.padding * 2.) * scale
            })
            .fold(base_column, f32::max);
        // Quantize width to avoid jitter as a counter gains/loses a digit.
        let mut desired = ((desired / (16. * scale)).ceil() * 16. * scale)
            .clamp(base_column, (m.max_width * scale).max(base_column));
        if g.columns == 1 && g.column_width > 0. {
            desired = g.column_width.clamp(80., g.screen[0] - 8.);
        }
        let mut count = if g.columns == 0 {
            1
        } else {
            g.columns.min(max)
        };
        // The vertical list must show every row in one strip: short rows
        // (name and value fit half the strip) share a line in pairs, which
        // saves height without shrinking the text.
        let pair = g.columns == 1;
        let (column, lines) = loop {
            let column =
                desired.min((g.screen[0] - 8. - padding * (count - 1) as f32) / count as f32);
            let content = |rows: usize| (cell_width(column, padding, rows) - 14. * scale).max(1.);
            let half = content(2);
            let short: Vec<bool> = rows
                .iter()
                .map(|r| pair && (r.name_width + r.value_width) * fit_scale + 18. * scale <= half)
                .collect();
            let lines: Vec<Line> = pack(&short)
                .into_iter()
                .map(|range| {
                    let width = content(range.len());
                    let mut height = 0f32;
                    for row in &mut rows[range.clone()] {
                        row.plan(width, &m, font, scale, fit_scale, |s| {
                            ui.calc_text_size(s)[0] * fit_scale
                        });
                        height = height.max(row.plan.as_ref().unwrap().height);
                    }
                    Line {
                        rows: range,
                        height,
                    }
                })
                .collect();
            let heights: Vec<_> = lines.iter().map(|l| l.height).collect();
            let next = columns(&heights, g.columns, limit, max);
            if next <= count {
                break (column, lines);
            }
            count = next;
        };
        let heights: Vec<_> = lines.iter().map(|l| l.height).collect();
        let chunks = partitions(&heights, count);
        let mut shown = Vec::with_capacity(chunks.len());
        let mut body_height = one.min(available);
        let mut placed = 0;
        for chunk in &chunks {
            let (n, h) = if g.columns == 1 {
                (chunk.len(), heights[chunk.clone()].iter().sum())
            } else {
                visible_rows(heights[chunk.clone()].iter().copied(), available, one)
            };
            placed += lines[chunk.clone()]
                .iter()
                .take(n)
                .map(|l| l.rows.len())
                .sum::<usize>();
            shown.push(n);
            body_height = body_height.max(h);
        }
        let hidden = rows.len() - placed;
        Self {
            column,
            width: count as f32 * column + (count - 1) as f32 * padding,
            height: (overhead + body_height).min(g.screen[1] - 8.),
            lines,
            chunks,
            shown,
            hidden,
            required_height: overhead + body_height,
            fit_scale,
            vertical: g.columns == 1,
            viewport: body_height.min(available),
        }
    }
}

/// Retains measured widths and wrapped ranges. Stable rows do not allocate,
/// measure text or reflow on each rendered frame.
#[derive(Default)]
pub(super) struct Card {
    rows: Vec<Row>,
    geometry: Option<Geometry>,
    layout: Option<Layout>,
    pub(super) column_width: f32,
}
impl Card {
    pub(super) fn draw<'a>(
        &mut self,
        ui: &Ui,
        position: [i32; 2],
        entries: impl IntoIterator<Item = (&'a str, String)>,
        compact: bool,
        hide_idle: bool,
        wanted_columns: usize,
    ) {
        let screen = ui.io().display_size;
        if screen.iter().any(|x| !x.is_finite() || *x < 80.) {
            return;
        }
        let g = Geometry {
            screen,
            font: ui.current_font_size(),
            font_id: ui.current_font() as *const _ as usize,
            compact,
            columns: wanted_columns,
            column_width: if wanted_columns == 1 {
                self.column_width
            } else {
                0.
            },
        };
        let font_changed = self
            .geometry
            .is_none_or(|old| old.font != g.font || old.font_id != g.font_id);
        let geometry_changed = self.geometry != Some(g);
        let mut changed = geometry_changed;
        if changed {
            for row in &mut self.rows {
                row.plan = None;
            }
        }
        let mut total = 0;
        let mut visible = 0;
        for (id, raw) in entries {
            total += 1;
            let state = status(id, &raw);
            if hide_idle && idle(&state) {
                continue;
            }
            if visible == self.rows.len() {
                self.rows.push(Row::default());
                changed = true;
            }
            changed |= self.rows[visible].update(id, state, font_changed, g.font / 16., |s| {
                ui.calc_text_size(s)[0]
            });
            visible += 1;
        }
        changed |= self.rows.len() != visible;
        self.rows.truncate(visible);
        if changed || self.layout.is_none() {
            let old = self.layout.as_ref();
            let mut factor = if geometry_changed {
                1.
            } else {
                old.map_or(1., |l| l.fit_scale)
            };
            let mut layout = Layout::new(&mut self.rows, g, ui, factor);
            // Stable counters keep the fitted size. Reclaim space if rows or
            // wrapped lines disappear; never resize the rest of ImGui.
            if factor < 1. && old.is_some_and(|l| layout.required_height < l.required_height - 0.1)
            {
                factor = 1.;
                layout = Layout::new(&mut self.rows, g, ui, factor);
            }
            if wanted_columns == 1 {
                layout = fit_height(layout, factor, screen[1] - 8., |f| {
                    Layout::new(&mut self.rows, g, ui, f)
                });
            }
            self.layout = Some(layout);
        }
        self.geometry = Some(g);
        paint(
            ui,
            position,
            &self.rows,
            self.layout.as_ref().unwrap(),
            compact,
            total,
        );
    }
}

/// The smallest text scale the vertical list shrinks to.
const MIN_FIT: f32 = 0.01;

/// The vertical list at the largest text scale (up to `factor`) whose height
/// fits `available`. Wrapping makes height a step function of the scale, so
/// one proportional guess overshoots (leaving the bottom of the screen
/// unused). It is a starting point, then bisection narrows the scale to 1%.
fn fit_height<L: Fitted>(
    layout: L,
    factor: f32,
    available: f32,
    mut build: impl FnMut(f32) -> L,
) -> L {
    if layout.required() <= available {
        return layout;
    }
    // `high` never fits; `best` is the largest scale known to fit.
    let mut high = factor;
    let mut last = layout;
    let mut best: Option<(f32, L)> = None;
    let mut guess = (factor * available / last.required()).clamp(MIN_FIT, factor);
    for _ in 0..12 {
        let candidate = build(guess);
        if candidate.required() <= available {
            best = Some((guess, candidate));
        } else {
            high = guess;
            last = candidate;
        }
        let low = best.as_ref().map(|(f, _)| *f);
        guess = match low {
            Some(low) if high - low <= high * 0.01 => break,
            Some(low) => (low + high) * 0.5,
            // Nothing fits yet: keep shrinking proportionally.
            None if guess <= MIN_FIT => break,
            None => (guess * available / last.required() * 0.999).max(MIN_FIT),
        };
    }
    best.map_or(last, |(_, layout)| layout)
}
trait Fitted {
    fn required(&self) -> f32;
}
impl Fitted for Layout {
    fn required(&self) -> f32 {
        self.required_height
    }
}

/// A feature that is only waiting (not connected, no data, idle): what
/// `hide_idle` leaves out.
fn idle(state: &Status) -> bool {
    state.badge && state.tone == Tone::Quiet
}
fn visible_rows(
    heights: impl Iterator<Item = f32> + Clone,
    available: f32,
    footer: f32,
) -> (usize, f32) {
    let total: f32 = heights.clone().sum();
    if total <= available {
        return (heights.count(), total);
    }
    let mut used = 0.;
    let mut count = 0;
    for height in heights {
        if used + height + footer > available {
            break;
        }
        used += height;
        count += 1;
    }
    (count, used + footer)
}

/// Draws the card. `compact` uses the smaller geometry; `hide_idle` leaves
/// out features that are only waiting (the header counts them as hidden).
#[cfg(test)]
pub(super) fn draw<'a>(
    ui: &Ui,
    position: [i32; 2],
    entries: impl IntoIterator<Item = (&'a str, String)>,
    compact: bool,
    hide_idle: bool,
    wanted_columns: usize,
) {
    Card::default().draw(ui, position, entries, compact, hide_idle, wanted_columns);
}

/// Draw at the fitted size without changing ImGui's font scale or opening its
/// implicit Debug window. These pointers belong to the active Ui frame only.
struct Text {
    list: *mut hudhook::imgui::sys::ImDrawList,
    font: *mut hudhook::imgui::sys::ImFont,
    size: f32,
}
impl Text {
    fn new(size: f32) -> Self {
        Self {
            list: unsafe { hudhook::imgui::sys::igGetBackgroundDrawList() },
            font: unsafe { hudhook::imgui::sys::igGetFont() },
            size,
        }
    }
    fn add_text(&self, position: [f32; 2], color: [f32; 4], text: impl AsRef<str>) {
        let text = text.as_ref();
        // Same draw list/atlas and UTF-8 bounds as DrawListMut::add_text. The
        // explicit size is the only difference; no game memory is involved.
        unsafe {
            hudhook::imgui::sys::ImDrawList_AddText_FontPtr(
                self.list,
                self.font,
                self.size,
                hudhook::imgui::sys::ImVec2 {
                    x: position[0],
                    y: position[1],
                },
                hudhook::imgui::ImColor32::from(color).to_bits(),
                text.as_ptr().cast(),
                text.as_ptr().add(text.len()).cast(),
                0.,
                std::ptr::null(),
            );
        }
    }
}

/// Draws one row inside its cell: `x` is (cell left, text left, cell right).
#[allow(clippy::too_many_arguments)]
fn paint_row(
    d: &hudhook::imgui::DrawListMut<'_>,
    text: &Text,
    row: &Row,
    m: &Metrics,
    x: [f32; 3],
    row_y: f32,
    font: f32,
    scale: f32,
    fit_scale: f32,
) {
    let [cell_x, label_x, right] = x;
    let plan = row.plan.as_ref().unwrap();
    let tone = row.tone.unwrap_or(Tone::Quiet);
    let first_y = row_y + m.row_inset * scale;
    d.add_circle(
        [cell_x + 3. * scale, first_y + font * 0.5],
        2. * scale,
        tone.text(),
    )
    .filled(true)
    .build();
    let line = font + 4. * scale;
    for (index, range) in plan.names.iter().enumerate() {
        text.add_text(
            [label_x, first_y + index as f32 * line],
            [0.85, 0.89, 0.94, 1.],
            &row.name[range.clone()],
        );
    }
    let value_y = if plan.stacked {
        first_y + plan.names.len() as f32 * line
    } else {
        first_y
    };
    let value_x = if plan.stacked {
        label_x
    } else {
        (right - row.value_width * fit_scale).max(label_x)
    };
    if row.badge {
        let pill_width = (row.value_width * fit_scale).min(right - value_x).max(0.);
        let pill = m.row_inset.min(3.) * scale;
        d.add_rect(
            [value_x, value_y - pill],
            [
                value_x + pill_width,
                value_y + (plan.values.len() - 1) as f32 * line + font + pill,
            ],
            tone.fill(),
        )
        .filled(true)
        .rounding(6. * scale)
        .build();
    }
    for (index, range) in plan.values.iter().enumerate() {
        text.add_text(
            [
                value_x + if row.badge { 8. * scale } else { 0. },
                value_y + index as f32 * line,
            ],
            tone.text(),
            &row.value[range.clone()],
        );
    }
}

fn paint(ui: &Ui, position: [i32; 2], rows: &[Row], layout: &Layout, compact: bool, total: usize) {
    let screen = ui.io().display_size;
    let m = metrics(compact, layout.vertical);
    let font = ui.current_font_size() * layout.fit_scale;
    let scale = font / 16.;
    let column = layout.column;
    let padding = m.padding * scale;
    let header = m.header * scale;
    let top = m.top * scale;
    let gap = padding;
    let hidden = layout.hidden;
    let width = layout.width;
    // Header text and the count pill sit centred in the header.
    let text_y = (header - font) * 0.5;
    let height = layout.height;
    let x = origin(position[0], screen[0], width);
    let y = origin(position[1], screen[1], height);
    crate::hud_editor::measure("hud", [width, height]);
    let d = ui.get_background_draw_list();
    let text = Text::new(font);
    d.add_rect(
        [x + 1., y + 5. * scale],
        [x + width + 1., y + height + 5. * scale],
        [0., 0., 0., 0.18],
    )
    .filled(true)
    .rounding(m.rounding * scale)
    .build();
    d.add_rect([x, y], [x + width, y + height], crate::theme::SURFACE)
        .filled(true)
        .rounding(m.rounding * scale)
        .build();
    d.add_rect([x, y], [x + width, y + height], crate::theme::BORDER)
        .rounding(m.rounding * scale)
        .build();
    // A small geometric mark keeps the header independent of icon/font assets.
    let size = m.mark * scale;
    let mark = [x + padding, y + (header - size) * 0.5];
    crate::theme::logo(&d, mark, size);
    let count = if rows.len() < total {
        format!("{}件 · 待機{}", rows.len() - hidden, total - rows.len())
    } else {
        format!("{}件", rows.len() - hidden)
    };
    let count_width = ui.calc_text_size(&count)[0] * layout.fit_scale + 16. * scale;
    let count_x = x + width - padding - count_width;
    d.add_rect(
        [count_x, y + text_y - 3. * scale],
        [count_x + count_width, y + text_y + font + 3. * scale],
        [0.60, 0.71, 0.86, 0.09],
    )
    .filled(true)
    .rounding(7. * scale)
    .build();
    text.add_text(
        [count_x + 8. * scale, y + text_y],
        [0.66, 0.73, 0.82, 1.],
        &count,
    );
    let title_x = mark[0] + size + 10. * scale;
    d.with_clip_rect([title_x, y], [count_x - 6. * scale, y + header], || {
        text.add_text(
            [title_x, y + text_y],
            crate::theme::TEXT,
            crate::brand::CLIENT_NAME,
        );
    });
    d.add_line(
        [x + padding, y + header],
        [x + width - padding, y + header],
        [0.65, 0.75, 0.9, 0.08],
    )
    .build();
    // Clip the full body to the card on small displays; no ImGui window is created.
    d.with_clip_rect([x, y + header], [x + width, y + height], || {
        let mut row_y = y + header + top;
        let mut label_x = x + padding + 14. * scale;
        for (c, (chunk, &visible)) in layout.chunks.iter().zip(&layout.shown).enumerate() {
            let left = x + c as f32 * (column + gap);
            row_y = y + header + top;
            label_x = left + padding + 14. * scale;
            d.with_clip_rect_intersect(
                [left, y + header + top],
                [left + column, y + header + top + layout.viewport],
                || {
                    for line in layout.lines[chunk.clone()].iter().take(visible) {
                        if row_y >= y + header + top + layout.viewport {
                            break;
                        }
                        let cells = line.rows.len();
                        let cell = cell_width(column, padding, cells);
                        if cells > 1 {
                            // A faint divider between the two cells of a line.
                            let divider = left + padding + cell + padding * 0.5;
                            d.add_line(
                                [divider, row_y + m.row_inset * scale],
                                [divider, row_y + line.height - m.row_inset * scale],
                                [0.65, 0.75, 0.9, 0.10],
                            )
                            .build();
                        }
                        for (k, row) in rows[line.rows.clone()].iter().enumerate() {
                            let cell_x = left + padding + k as f32 * (cell + padding);
                            let text_x = cell_x + 14. * scale;
                            let right = cell_x + cell;
                            paint_row(
                                &d,
                                &text,
                                row,
                                &m,
                                [cell_x, text_x, right],
                                row_y,
                                font,
                                scale,
                                layout.fit_scale,
                            );
                        }
                        row_y += line.height;
                    }
                },
            );
        }
        if rows.is_empty() {
            text.add_text(
                [x + padding, row_y + m.row_inset * scale],
                [0.51, 0.58, 0.68, 1.],
                "表示中の機能はありません",
            );
        } else if hidden > 0 {
            // Under the last column's rows (each column kept room for it).
            text.add_text(
                [label_x, row_y + m.row_inset * scale],
                [0.51, 0.58, 0.68, 1.],
                format!("ほか {hidden} 件"),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vertical_list_keeps_every_row_visible_without_input_or_font_side_effects() {
        let _serial = super::super::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for pixels in [12., 16., 28.] {
            let mut ctx = super::super::fonts::test_context(pixels);
            let mut card = Card {
                column_width: 360.,
                ..Default::default()
            };
            for size in [[1280., 720.], [2560., 1440.], [800., 600.], [320., 240.]] {
                for count in [35, 70] {
                    ctx.io_mut().display_size = size;
                    ctx.io_mut().delta_time = 1. / 60.;
                    ctx.io_mut().mouse_wheel = -10.;
                    let ui = ctx.frame();
                    let original_font = ui.current_font_size();
                    card.draw(
                        ui,
                        [10, 10],
                        super::super::preview::OVERFLOW_HUD
                            .iter()
                            .cycle()
                            .take(count)
                            .map(|&(id, text)| (id, text.into())),
                        true,
                        false,
                        1,
                    );
                    let layout = card.layout.as_ref().unwrap();
                    assert_eq!(layout.chunks.len(), 1);
                    // Every row is on a shown line (short ones two to a line).
                    assert_eq!(layout.shown[0], layout.lines.len());
                    assert_eq!(
                        layout.lines.iter().map(|l| l.rows.len()).sum::<usize>(),
                        count
                    );
                    assert!(layout.lines.len() < count, "short rows share lines");
                    assert_eq!(layout.hidden, 0);
                    assert!(
                        layout.required_height <= size[1] - 8. + 0.1,
                        "{size:?} / {pixels} / {count} / {}",
                        layout.required_height
                    );
                    assert_eq!(layout.width, 360f32.min(size[0] - 8.));
                    assert_eq!(ui.current_font_size(), original_font);
                    if pixels == 16. && size[1] == 1440. && count == 35 {
                        assert_eq!(layout.fit_scale, 1.);
                    }
                    for row in &card.rows {
                        let plan = row.plan.as_ref().unwrap();
                        assert_eq!(
                            plan.values
                                .iter()
                                .map(|r| &row.value[r.clone()])
                                .collect::<String>(),
                            row.value
                        );
                        assert!(plan
                            .names
                            .iter()
                            .all(|r| ui.calc_text_size(&row.name[r.clone()])[0]
                                * layout.fit_scale
                                <= plan.content + 0.1));
                        assert!(plan.values.iter().all(|r| ui
                            .calc_text_size(&row.value[r.clone()])[0]
                            * layout.fit_scale
                            <= plan.content
                                - if row.badge {
                                    pixels * layout.fit_scale
                                } else {
                                    0.
                                }
                                + 0.1));
                    }
                    let draw = ctx.render();
                    assert_eq!(
                        draw.draw_lists_count(),
                        1,
                        "must not activate ImGui's implicit Debug window"
                    );
                    assert!(draw
                        .draw_lists()
                        .flat_map(|d| d.vtx_buffer())
                        .all(|v| v.pos.iter().all(|x| x.is_finite())));
                }
            }
        }
    }
    #[test]
    fn short_neighbouring_rows_pair_up_in_order() {
        assert_eq!(pack(&[]), Vec::<Range<usize>>::new());
        assert_eq!(pack(&[true]), vec![0..1]);
        assert_eq!(pack(&[true, true, true]), vec![0..2, 2..3]);
        // A long row breaks a pair; order is never changed.
        assert_eq!(
            pack(&[true, false, true, true, false, true]),
            vec![0..1, 1..2, 2..4, 4..5, 5..6]
        );
        // Two cells and their gap fill exactly the width of one.
        let (column, padding) = (360., 10.);
        assert_eq!(
            2. * cell_width(column, padding, 2) + padding,
            cell_width(column, padding, 1)
        );
    }

    #[test]
    fn vertical_fit_uses_the_largest_scale_that_fits_despite_wrapping_steps() {
        struct Fake(f32);
        impl Fitted for Fake {
            fn required(&self) -> f32 {
                self.0
            }
        }
        // 40 px of header plus rows whose wrapped lines jump in steps: at
        // scales above 0.8 every row wraps to a second line.
        let height = |f: f32| Fake(40. + 50. * 20. * f * if f > 0.8 { 2. } else { 1. });
        let calls = std::cell::Cell::new(0);
        let fitted = fit_height(height(1.), 1., 700., |f| {
            calls.set(calls.get() + 1);
            height(f)
        });
        // The proportional guess alone (700/2040 = 0.34) leaves 360 px unused;
        // bisection lands within 1% below the best scale, 0.66.
        assert!(fitted.0 <= 700. && fitted.0 > 690., "{}", fitted.0);
        assert!(calls.get() <= 12);
        // Already fitting: unchanged, nothing rebuilt.
        calls.set(0);
        let small = fit_height(Fake(300.), 1., 700., |f| {
            calls.set(calls.get() + 1);
            height(f)
        });
        assert_eq!((small.0, calls.get()), (300., 0));
        // Never fits: the smallest scale's layout, without looping forever.
        let never = fit_height(Fake(5000.), 1., 10., |f| Fake(100. + f));
        assert!(never.0 >= 100. && never.0 <= 100. + 2. * MIN_FIT);
    }

    #[test]
    fn wrapping_preserves_every_utf8_character_without_overflow_or_ellipsis() {
        let measure = |s: &str| {
            s.chars()
                .map(|c| if c.is_ascii() { 7. } else { 16. })
                .sum::<f32>()
        };
        for text in [
            "brown_terracotta / 座標 485 / 96 / 98",
            "非常に長い日本語の状態表示😀",
            "PerformanceProfiler",
            "",
            "a b c d",
        ] {
            for width in [16., 25., 80., 200., 1000.] {
                let ranges = wrap(text, width, measure);
                let joined = ranges.iter().map(|r| &text[r.clone()]).collect::<String>();
                assert_eq!(joined, text);
                assert!(
                    ranges.iter().all(|r| measure(&text[r.clone()]) <= width),
                    "{text} / {width}"
                );
            }
        }
        // Even an impossibly narrow column must make forward progress.
        assert_eq!(wrap("あ😀", 0., measure), vec![0..3, 3..7]);
        assert_eq!(tallest(&[100., 100., 20., 20., 20., 20.], 2), 180.);
    }

    #[test]
    fn unchanged_rows_reuse_widths_and_wrapped_ranges() {
        let mut row = Row::default();
        let calls = std::cell::Cell::new(0);
        let measure = |s: &str| {
            calls.set(calls.get() + 1);
            s.chars().count() as f32 * 8.
        };
        assert!(row.update(
            "frametime",
            status("frametime", "平均 8.6 ms / 最大 15.1 ms"),
            true,
            1.,
            measure
        ));
        row.plan(200., &COMPACT, 16., 1., 1., measure);
        assert!(row.plan.as_ref().unwrap().stacked);
        calls.set(0);
        assert!(!row.update(
            "frametime",
            status("frametime", "平均 8.6 ms / 最大 15.1 ms"),
            false,
            1.,
            measure
        ));
        row.plan(200., &COMPACT, 16., 1., 1., measure);
        assert_eq!(calls.get(), 0);
        assert!(row.update(
            "frametime",
            status("frametime", "平均 9.6 ms / 最大 15.1 ms"),
            false,
            1.,
            measure
        ));
        assert_eq!(calls.get(), 1, "only the changed value needs measuring");
        row.plan(200., &COMPACT, 16., 1., 1., measure);
        assert_eq!(row.name, "FrameTime");
    }

    #[test]
    fn screenshot_rows_fit_with_full_names_values_and_balanced_columns() {
        let _serial = super::super::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for pixels in [12., 16., 28.] {
            let mut ctx = super::super::fonts::test_context(pixels);
            let mut card = Card::default();
            for size in [
                [1280., 720.],
                [2560., 1440.],
                [800., 600.],
                [320., 240.],
                [1280., 720.],
            ] {
                for compact in [true, false] {
                    ctx.io_mut().display_size = size;
                    ctx.io_mut().delta_time = 1. / 60.;
                    let ui = ctx.frame();
                    card.draw(
                        ui,
                        [-10, -10],
                        super::super::preview::OVERFLOW_HUD.map(|(id, text)| (id, text.into())),
                        compact,
                        true,
                        0,
                    );
                    let layout = card.layout.as_ref().unwrap();
                    assert!(layout.width <= size[0] - 8. && layout.height <= size[1] - 8.);
                    assert_eq!(card.rows.len(), 33);
                    if (compact && size[0] >= 1280.) || size[0] >= 1920. {
                        assert_eq!(
                            layout.hidden, 0,
                            "size={size:?} font={pixels} compact={compact}"
                        );
                    }
                    for row in &card.rows {
                        let plan = row.plan.as_ref().unwrap();
                        assert_eq!(
                            plan.names
                                .iter()
                                .map(|r| &row.name[r.clone()])
                                .collect::<String>(),
                            row.name
                        );
                        assert_eq!(
                            plan.values
                                .iter()
                                .map(|r| &row.value[r.clone()])
                                .collect::<String>(),
                            row.value
                        );
                        for range in &plan.names {
                            assert!(
                                ui.calc_text_size(&row.name[range.clone()])[0]
                                    <= plan.content + 0.1
                            );
                        }
                        for range in &plan.values {
                            assert!(
                                ui.calc_text_size(&row.value[range.clone()])[0]
                                    <= plan.content - if row.badge { pixels } else { 0. } + 0.1
                            );
                        }
                        if !plan.stacked {
                            assert!(
                                row.name_width + row.value_width + 18. * pixels / 16.
                                    <= plan.content
                            );
                        }
                    }
                    let data = ctx.render();
                    assert!(data.total_vtx_count > 0);
                    assert!(data
                        .draw_lists()
                        .flat_map(|d| d.vtx_buffer())
                        .all(|v| v.pos.iter().all(|p| p.is_finite())));
                }
            }
            // Changing hide-idle on the same cached card must restore all rows.
            let ui = ctx.frame();
            card.draw(
                ui,
                [10, 10],
                super::super::preview::OVERFLOW_HUD.map(|(id, text)| (id, text.into())),
                true,
                false,
                0,
            );
            assert_eq!(card.rows.len(), 35);
            assert_eq!(card.layout.as_ref().unwrap().hidden, 0);
            ctx.render();
        }
    }
    #[test]
    fn concise_labels_do_not_change_the_source_status_or_module_identity() {
        let original = "インベントリ中の移動が有効";
        let active = status("inventorymove", original);
        assert_eq!(active.text, "移動可能");
        assert_eq!(active.tone, Tone::Ready);
        assert_eq!(original, "インベントリ中の移動が有効");
        assert_eq!(status("coordinates", "（ワールド未接続）").text, "未接続");
        assert_eq!(status("autofish", "一時停止: メニュー").tone, Tone::Warning);
        assert_eq!(status("my_patch", "独自の状態").text, "独自の状態");
        assert_eq!(
            status("my_patch", "状態の要約\n詳細情報").text,
            "状態の要約"
        );
    }
    #[test]
    fn long_lists_take_more_columns_instead_of_running_down_the_screen() {
        let rows = [20.; 30]; // 600 high in one column
                              // Automatic: the fewest columns within the limit.
        assert_eq!(columns(&rows, 0, 700., 3), 1);
        assert_eq!(columns(&rows, 0, 300., 3), 2);
        assert_eq!(columns(&rows, 0, 200., 3), 3);
        // Still too tall at the most columns: the most.
        assert_eq!(columns(&rows, 0, 50., 3), 3);
        // Only what fits across the screen.
        assert_eq!(columns(&rows, 0, 200., 2), 2);
        // A fixed number, within what fits.
        assert_eq!(columns(&rows, 2, 10_000., 3), 2);
        assert_eq!(columns(&rows, 3, 10_000., 1), 1);
        assert_eq!(columns(&[], 0, 100., 3), 1);
        // Filled top to bottom: 30 rows in 3 columns of 10.
        assert_eq!(tallest(&rows, 3), 200.);
        assert_eq!(tallest(&rows[..7], 2), 80.);
    }

    #[test]
    fn long_lists_fit_the_screen_and_reserve_a_remaining_count() {
        assert_eq!(origin(-20, 1280., 320.), 940.);
        assert_eq!(origin(500, 400., 320.), 80.);
        assert_eq!(visible_rows([30.; 6].into_iter(), 200., 30.), (6, 180.));
        assert_eq!(visible_rows([30.; 6].into_iter(), 100., 30.), (2, 90.));
        assert_eq!(
            visible_rows([50., 30., 50.].into_iter(), 110., 30.),
            (2, 110.)
        );
    }
}
