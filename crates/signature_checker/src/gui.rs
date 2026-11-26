use crate::theme::*;
mod clipboard;
use eframe::egui::{self, RichText};
use mio_signature_check::{
    audit_file, cli::Options, default_exe, save_report, CheckState, MatchState, Outcome, Report,
    Role, SIGNATURE_COUNT,
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    All,
    Missing,
    Candidates,
    Core,
    Optional,
    Address,
}
type Job = Receiver<Result<Box<Report>, String>>;
pub struct App {
    exe: String,
    baseline: String,
    export: Option<PathBuf>,
    job: Option<Job>,
    report: Option<Box<Report>>,
    message: Option<String>,
    search: String,
    filter: Filter,
    problems_first: bool,
    autorun: bool,
}
impl App {
    pub fn new(options: Result<Options, String>) -> Self {
        let mut app = Self {
            exe: default_exe().to_string_lossy().into(),
            baseline: String::new(),
            export: None,
            job: None,
            report: None,
            message: None,
            search: String::new(),
            filter: Filter::All,
            problems_first: true,
            autorun: false,
        };
        match options {
            Ok(options) => {
                app.autorun = options.exe.is_some() && !options.help;
                if let Some(exe) = options.exe {
                    app.exe = exe.to_string_lossy().into();
                }
                if let Some(baseline) = options.baseline {
                    app.baseline = baseline.to_string_lossy().into();
                }
                app.export = options.out;
                if options.help {
                    app.message = Some(mio_signature_check::cli::HELP.into());
                }
            }
            Err(error) => app.message = Some(error),
        }
        app
    }
    fn start(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        let exe = PathBuf::from(self.exe.trim());
        let baseline =
            (!self.baseline.trim().is_empty()).then(|| PathBuf::from(self.baseline.trim()));
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("Mio signature audit".into())
            .spawn(move || {
                let _ = tx.send(audit_file(&exe, baseline.as_deref()).map(Box::new));
                ctx.request_repaint();
            }) {
            Ok(_) => {
                self.job = Some(rx);
                self.report = None;
                self.message = None;
            }
            Err(error) => {
                self.message = Some(format!("検証スレッドを開始できませんでした: {error}"));
            }
        }
    }
    fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let result = match job.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Disconnected) => {
                Err("検証スレッドが終了しました。入力ファイルを確認して再試行してください。".into())
            }
            Err(TryRecvError::Empty) => return,
        };
        self.job = None;
        match result {
            Ok(report) => {
                if let Some(path) = self.export.take() {
                    self.message = Some(match save_report(&path, &report) {
                        Ok(()) => format!("JSONを保存しました: {}", path.display()),
                        Err(error) => error,
                    });
                }
                self.report = Some(report);
            }
            Err(error) => self.message = Some(error),
        }
    }
    fn draw(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG).inner_margin(22)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.heading("Mio Signature Checker");
                ui.label(RichText::new("Minecraftの更新後に、クライアントのシグネチャを実行ファイル上で照合します。").color(MUTED));
                ui.label(RichText::new(format!("組み込み検証定義: {SIGNATURE_COUNT}件（クライアントと共有）")).color(MUTED).size(12.));
                ui.add_space(8.);
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add_enabled_ui(self.job.is_none(), |ui| {
                        ui.label("Minecraft.Windows.exe（ここへEXEをドロップできます）");
                        if path_field(ui, &mut self.exe, false) { self.report = None; }
                        ui.label("比較用JSON（任意・更新前にこのツールで保存したもの）");
                        if path_field(ui, &mut self.baseline, true) { self.report = None; }
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(RichText::new("検証を開始").color(BG)).fill(ACCENT)).clicked() { self.start(ui.ctx()); }
                            if ui.button("比較なし").clicked() { self.baseline.clear(); self.report = None; }
                        });
                    });
                    ui.label(RichText::new("実行ファイルは読み取り専用です。ゲームの起動・注入・メモリ変更は行いません。").color(MUTED).size(12.));
                });
                if self.job.is_some() {
                    ui.horizontal(|ui| { ui.spinner(); ui.label("EXEを読み込み、全シグネチャと静的な整合性を検証しています…"); });
                }
                if let Some(message) = &self.message { ui.label(RichText::new(message).color(WARN)); }
                if let Some(report) = self.report.as_deref() {
                    let color = match report.outcome { Outcome::StaticChecksPassed => GOOD, Outcome::IssuesFound => ERROR, Outcome::ReviewRequired => WARN };
                    card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(report.outcome.label()).color(color).size(22.).strong());
                        ui.label(format!("{} signatures / {} ms / PE識別値: {}", report.signatures.len(), report.elapsed_ms, report.recorded_build.as_deref().unwrap_or("未解析ビルド")));
                        ui.label(format!("未検出 {} / 一意 {} / 複数候補 {}", report.signatures.iter().filter(|s|s.state==MatchState::Missing).count(),report.signatures.iter().filter(|s|s.state==MatchState::Unique).count(),report.signatures.iter().filter(|s|s.state==MatchState::Candidates).count()));
                        ui.add(egui::Label::new(&report.exe).wrap());
                        ui.label(format!("timestamp=0x{:08X}  image=0x{:X}  checksum=0x{:X}", report.identity.timestamp, report.identity.image_size, report.identity.checksum));
                        ui.label(RichText::new("一致してもABI・オブジェクト寿命・全機能の動作までは保証できません。未解析ビルドは実機確認が必要です。").color(WARN));
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("JSONを保存…").clicked() {
                                if let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).set_file_name("mio_signature_report.json").save_file() {
                                    self.message = Some(match save_report(&path, report) { Ok(()) => format!("保存しました: {}", path.display()), Err(error) => error });
                                }
                            }
                            if ui.button("全結果をコピー").on_hover_text("全シグネチャ・整合性検証・差分をテキストでコピー").clicked() {
                                ui.ctx().copy_text(clipboard::report_text(report));
                                self.message = Some("検証結果をテキストでコピーしました。".into());
                            }
                            if ui.button("JSONをコピー").clicked() {
                                match serde_json::to_string_pretty(report) { Ok(text) => { ui.ctx().copy_text(text); self.message = Some("検証結果をJSONでコピーしました。".into()); }, Err(error) => self.message = Some(error.to_string()) }
                            }
                            ui.label(RichText::new("既存ファイルには上書きしません").color(MUTED).size(12.));
                        });
                    });
                    egui::CollapsingHeader::new("命令・アドレス・フィールドの整合性").default_open(true).show(ui, |ui| {
                        for check in &report.checks {
                            ui.push_id(&check.name, |ui| {
                            let color = match check.state { CheckState::Passed => GOOD, CheckState::Failed => ERROR, CheckState::Review => WARN };
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(format!("{:?}", check.state)).color(color));
                                ui.label(&check.name).on_hover_text(format!("{}\n{:X?}", check.detail, check.rvas_or_values));
                                if ui.small_button("コピー").on_hover_text("判定・説明・全フィールド値をコピー").clicked() {
                                    ui.ctx().copy_text(clipboard::check_text(check));
                                }
                            });
                            if check.state != CheckState::Passed { ui.add(egui::Label::new(&check.detail).wrap()); }
                            });
                        }
                    });
                    if let Some(baseline) = &report.baseline {
                        egui::CollapsingHeader::new(format!("更新前との差分: {}件", report.changes.len())).show(ui, |ui| {
                            ui.add(egui::Label::new(baseline).wrap());
                            for change in &report.changes { ui.add(egui::Label::new(format!("{}: {}", change.name, change.detail)).wrap()); }
                            if report.changes.is_empty() { ui.label("一致数・RVA・パターン・整合性検証の変化はありません。"); }
                        });
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.search).desired_width(220.).hint_text("名前・用途で検索"));
                        egui::ComboBox::from_id_salt("result-filter").selected_text(format!("{:?}", self.filter)).show_ui(ui, |ui| {
                            for filter in [Filter::All, Filter::Missing, Filter::Candidates, Filter::Core, Filter::Optional, Filter::Address] { ui.selectable_value(&mut self.filter, filter, format!("{filter:?}")); }
                        });
                        ui.checkbox(&mut self.problems_first, "問題を先頭に");
                    });
                    ui.label(RichText::new("Multiple candidatesは、整合性検証で絞り込める場合があります。名前・RVAの右クリック、または行末の「コピー」から名前・パターン・全RVAをコピーできます。").color(MUTED).size(12.));
                    let query = self.search.to_lowercase();
                    let mut rows: Vec<_> = report.signatures.iter().filter(|s| {
                        let matches = match self.filter { Filter::All => true, Filter::Missing => s.state == MatchState::Missing, Filter::Candidates => s.state == MatchState::Candidates, Filter::Core => s.role == Role::Core, Filter::Optional => s.role == Role::Optional, Filter::Address => s.role == Role::Address };
                        matches && (s.name.to_lowercase().contains(&query) || s.purpose.to_lowercase().contains(&query))
                    }).collect();
                    if self.problems_first { rows.sort_by_key(|s| (match s.state { MatchState::Missing => 0, MatchState::Candidates => 1, MatchState::Unique => 2 }, &s.name)); }
                    egui::ScrollArea::horizontal().id_salt("signature-table-scroll").show(ui, |ui| {
                        egui::Grid::new("signature-results").striped(true).min_col_width(64.).show(ui, |ui| {
                            for name in ["State", "Role", "Count", "Signature", "First RVA", "コピー"] { ui.strong(name); } ui.end_row();
                            for s in rows {
                                let (label, color) = match s.state { MatchState::Missing => ("MISSING", ERROR), MatchState::Unique => ("MATCH", GOOD), MatchState::Candidates => ("CANDIDATES", WARN) };
                                ui.label(RichText::new(label).color(color));
                                ui.label(format!("{:?}", s.role));
                                ui.label(format!("{}{}", s.rvas.len(), if s.capped { "+" } else { "" }));
                                ui.add_sized([360., 22.], egui::Label::new(RichText::new(&s.name).monospace().size(12.)).truncate()).on_hover_text(format!("{}\n\n{}\n\nRVAs: {:X?}", s.purpose, s.pattern, s.rvas)).context_menu(|ui| clipboard::signature_menu(ui, s));
                                ui.monospace(s.rvas.first().map(|v| format!("0x{v:08X}")).unwrap_or_else(|| "-".into())).context_menu(|ui| clipboard::signature_menu(ui, s));
                                ui.push_id(("copy", &s.name), |ui| {
                                    ui.menu_button("コピー", |ui| clipboard::signature_menu(ui, s));
                                });
                                ui.end_row();
                            }
                        });
                    });
                } else if self.job.is_none() {
                    ui.add_space(16.); ui.label("更新前にJSONを保存し、更新後のEXEと比較できます。");
                }
            });
        });
    }
}
fn path_field(ui: &mut egui::Ui, value: &mut String, json: bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("参照…").clicked() {
                let dialog = if json {
                    rfd::FileDialog::new().add_filter("JSON", &["json"])
                } else {
                    rfd::FileDialog::new().add_filter("Executable", &["exe"])
                };
                if let Some(path) = dialog.pick_file() {
                    *value = path.to_string_lossy().into();
                    changed = true;
                }
            }
            changed |= ui
                .add_sized(
                    [ui.available_width(), 36.],
                    egui::TextEdit::singleline(value).desired_width(f32::INFINITY),
                )
                .changed();
        });
    });
    changed
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        if self.autorun {
            self.autorun = false;
            self.start(ctx);
        }
        if self.job.is_none() {
            if let Some(path) =
                ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))
            {
                self.exe = path.to_string_lossy().into();
                self.start(ctx);
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(if self.job.is_some() {
            80
        } else {
            1000
        }));
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.draw(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn sample_report() -> Box<Report> {
        use mio_signature_check::{Check, Identity, SignatureResult};
        Box::new(Report {
            schema_version: 1,
            exe: "D:/日本語のフォルダー/Minecraft.Windows.exe".into(),
            identity: Identity {
                machine: 0x8664,
                timestamp: 1,
                image_size: 0x4000,
                checksum: 0,
                image_base: 0x140000000,
                file_size: 4096,
            },
            recorded_build: None,
            elapsed_ms: 123,
            outcome: Outcome::IssuesFound,
            note: mio_signature_check::NOTE.into(),
            signatures: vec![SignatureResult {
                name: "A_VERY_LONG_SIGNATURE_NAME_FOR_GUI_LAYOUT_VALIDATION".into(),
                purpose: "用途・機能の説明".into(),
                pattern: "48 8B ??".into(),
                role: Role::Core,
                state: MatchState::Missing,
                capped: false,
                rvas: vec![],
            }],
            checks: vec![Check {
                name: "Static field check".into(),
                state: CheckState::Failed,
                detail: "検証できませんでした。".repeat(30),
                rvas_or_values: Default::default(),
            }],
            baseline: Some("previous.json".into()),
            changes: vec![],
        })
    }
    #[test]
    fn idle_error_and_busy_views_render_at_small_sizes_and_high_dpi() {
        for (size, scale) in [
            ([760., 560.], 1.),
            ([1040., 780.], 1.5),
            ([1280., 900.], 2.),
        ] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let mut app = App::new(Ok(Options::default()));
            for case in 0..5 {
                app.message = (case == 1).then(|| "テスト: ファイルが見つかりません。".repeat(8));
                let (_tx, rx) = mpsc::channel();
                app.job = (case == 2).then_some(rx);
                app.report = (case >= 3).then(sample_report);
                if case == 4 {
                    app.filter = Filter::Candidates;
                }
                let mut input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
                    ..Default::default()
                };
                input
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .native_pixels_per_point = Some(scale);
                let output = ctx.run_ui(input, |ui| {
                    app.draw(ui);
                    assert!(ui.min_rect().right() <= size[0] + 1.);
                });
                assert!(!ctx
                    .tessellate(output.shapes.clone(), output.pixels_per_point)
                    .is_empty());
                output.drop_without_applying_deltas();
            }
        }
    }
}
