//! Clipboard payloads are built only on demand; exporting never mutates a report.
use eframe::egui;
use mio_signature_check::{summary, Check, Report, SignatureResult};
use std::fmt::Write;

fn rvas_text(signature: &SignatureResult) -> String {
    signature
        .rvas
        .iter()
        .map(|at| format!("0x{at:08X}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn signature_text(signature: &SignatureResult) -> String {
    format!(
        "{}\nState: {:?}\nRole: {:?}\nMatches: {}{}\nPattern: {}\nRVAs:\n{}\nPurpose: {}\n",
        signature.name,
        signature.state,
        signature.role,
        signature.rvas.len(),
        if signature.capped {
            "+ (上限到達)"
        } else {
            ""
        },
        signature.pattern,
        if signature.rvas.is_empty() {
            "-".into()
        } else {
            rvas_text(signature)
        },
        signature.purpose,
    )
}

pub(super) fn check_text(check: &Check) -> String {
    let mut text = format!("{}: {:?}\n{}\n", check.name, check.state, check.detail);
    for (name, value) in &check.rvas_or_values {
        let _ = writeln!(text, "{name}: 0x{value:X}");
    }
    text
}

pub(super) fn report_text(report: &Report) -> String {
    let mut text = format!(
        "Mio Signature Checker\n{}\n{}\nPE: timestamp=0x{:08X} image_size=0x{:X} checksum=0x{:X} image_base=0x{:X}\n\n",
        report.exe, summary(report), report.identity.timestamp,
        report.identity.image_size, report.identity.checksum, report.identity.image_base,
    );
    for signature in &report.signatures {
        text.push_str(&signature_text(signature));
        text.push('\n');
    }
    text.push_str("Static checks\n");
    for check in &report.checks {
        text.push_str(&check_text(check));
        text.push('\n');
    }
    if let Some(baseline) = &report.baseline {
        let _ = writeln!(text, "Baseline: {baseline}");
    }
    for change in &report.changes {
        let _ = writeln!(text, "CHANGED {}: {}", change.name, change.detail);
    }
    text
}

fn copy_button(ui: &mut egui::Ui, label: &str, text: impl FnOnce() -> String) -> egui::Response {
    let response = ui.button(label);
    if response.clicked() {
        ui.ctx().copy_text(text());
        ui.close();
    }
    response
}

pub(super) fn signature_menu(ui: &mut egui::Ui, signature: &SignatureResult) {
    copy_button(ui, "名前をコピー", || signature.name.clone());
    copy_button(ui, "パターンをコピー", || signature.pattern.clone());
    ui.add_enabled_ui(!signature.rvas.is_empty(), |ui| {
        copy_button(ui, "全RVAをコピー", || rvas_text(signature));
    });
    ui.separator();
    copy_button(ui, "この行をコピー", || signature_text(signature));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mio_signature_check::{Change, CheckState, MatchState, Role};

    #[test]
    fn copy_text_keeps_all_candidates_cap_unicode_and_static_check_details() {
        let signature = SignatureResult {
            name: "TEST_SIGNATURE".into(),
            purpose: "描画カメラ・日本語".into(),
            pattern: "48 8B ??".into(),
            role: Role::Optional,
            state: MatchState::Candidates,
            capped: true,
            rvas: (0..256).map(|n| 0x1000 + n * 16).collect(),
        };
        let text = signature_text(&signature);
        assert!(text.contains("256+ (上限到達)"));
        assert!(text.contains("48 8B ??"));
        assert!(text.contains("描画カメラ・日本語"));
        let addresses = rvas_text(&signature);
        assert_eq!(addresses.lines().count(), 256);
        assert!(addresses.ends_with("0x00001FF0"));
        let mut report = super::super::tests::sample_report();
        report.signatures = vec![signature];
        report.checks = vec![Check {
            name: "Camera".into(),
            state: CheckState::Passed,
            detail: "説明".into(),
            rvas_or_values: [("origin".into(), 0x64c)].into(),
        }];
        report.changes.push(Change {
            name: "Camera".into(),
            detail: "field changed".into(),
        });
        let text = report_text(&report);
        assert!(text.contains("origin: 0x64C"));
        assert!(text.contains("Baseline: previous.json"));
        assert!(text.contains("CHANGED Camera: field changed"));
        assert!(text.contains("0x00001FF0"));
        report.signatures[0].rvas.clear();
        report.signatures[0].capped = false;
        report.signatures[0].state = MatchState::Missing;
        assert!(signature_text(&report.signatures[0]).contains("RVAs:\n-\n"));
    }

    #[test]
    fn clicking_copy_emits_the_exact_clipboard_command() {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600., 400.),
            )),
            ..Default::default()
        };
        let mut button = egui::Rect::NOTHING;
        let frame = ctx.run_ui(input(), |ui| {
            button = copy_button(ui, "パターンをコピー", || "48 8B ??".into()).rect;
        });
        assert!(!frame
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::CopyText(_))));
        frame.drop_without_applying_deltas();
        for pressed in [true, false] {
            let mut raw = input();
            raw.events = vec![
                egui::Event::PointerMoved(button.center()),
                egui::Event::PointerButton {
                    pos: button.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let output = ctx.run_ui(raw, |ui| {
                copy_button(ui, "パターンをコピー", || "48 8B ??".into());
            });
            if !pressed {
                assert!(output
                    .platform_output
                    .commands
                    .contains(&egui::OutputCommand::CopyText("48 8B ??".into())));
            }
            output.drop_without_applying_deltas();
        }
    }
}
