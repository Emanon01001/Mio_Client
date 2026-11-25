use super::*;

fn put16(b: &mut [u8], at: usize, value: u16) {
    b[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn fixture() -> Vec<u8> {
    let mut b = vec![0u8; 0x1600];
    b[..2].copy_from_slice(b"MZ");
    put32(&mut b, 0x3c, 0x80);
    b[0x80..0x84].copy_from_slice(b"PE\0\0");
    put16(&mut b, 0x84, 0x8664);
    put16(&mut b, 0x86, 3);
    put16(&mut b, 0x94, 0xf0);
    put16(&mut b, 0x98, 0x20b);
    b[0xb0..0xb8].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
    put32(&mut b, 0x98 + 56, 0x8000);
    put32(&mut b, 0x98 + 60, 0x200);
    for (index, (name, rva, raw, size, flags)) in [
        (".text", 0x1000, 0x200, 0x1000, 0x60000020),
        (".rdata", 0x6000, 0x1200, 0x200, 0x40000040),
        (".data", 0x7000, 0x1400, 0x200, 0xc0000040),
    ]
    .into_iter()
    .enumerate()
    {
        let at = 0x188 + index * 40;
        b[at..at + name.len()].copy_from_slice(name.as_bytes());
        put32(&mut b, at + 8, size);
        put32(&mut b, at + 12, rva);
        put32(&mut b, at + 16, size);
        put32(&mut b, at + 20, raw);
        put32(&mut b, at + 36, flags);
    }
    b
}
fn literal(pattern: &str) -> Vec<u8> {
    pattern
        .split_whitespace()
        .map(|s| {
            if s == "??" || s == "?" {
                0
            } else {
                u8::from_str_radix(s, 16).unwrap()
            }
        })
        .collect()
}

#[test]
fn checked_pe_rejects_truncation_architecture_ranges_and_overlaps() {
    let valid = fixture();
    assert!(pe::Pe::parse(&valid).is_ok());
    for len in (0..valid.len()).step_by(17) {
        assert!(pe::Pe::parse(&valid[..len]).is_err(), "len={len}");
    }
    for (offset, value) in [
        (0x3c, u32::MAX),
        (0x98 + 60, 0),
        (0x188 + 20, u32::MAX),
        (0x188 + 40 + 12, 0x1000),
        (0x188 + 40 + 20, 0x200),
        (0x98 + 56, 0x1000),
    ] {
        let mut b = valid.clone();
        put32(&mut b, offset, value);
        assert!(pe::Pe::parse(&b).is_err(), "offset={offset:x}");
    }
    let mut b = valid.clone();
    put16(&mut b, 0x84, 0x14c);
    assert!(pe::Pe::parse(&b).is_err());
    let mut b = valid;
    put16(&mut b, 0x98, 0x10b);
    assert!(pe::Pe::parse(&b).is_err());
}
#[test]
fn rva_reads_reject_virtual_only_bytes_and_cross_section_ranges() {
    let mut b = fixture();
    put32(&mut b, 0x188 + 8, 0x1800);
    let pe = pe::Pe::parse(&b).unwrap();
    assert!(pe.bytes(0x1000, 8).is_some());
    assert!(pe.bytes(0x1fff, 2).is_none());
    assert!(pe.bytes(0x2200, 1).is_none());
    assert!(pe.bytes(usize::MAX, 2).is_none());
}
#[test]
fn audit_counts_only_executable_bytes_and_reports_ambiguous_and_missing() {
    let mut b = fixture();
    let pattern = literal(signatures::ITEM_MAX_DAMAGE_SIGNATURE);
    for at in [0x220, 0x240, 0x1220] {
        b[at..at + pattern.len()].copy_from_slice(&pattern);
    }
    let skin = literal(signatures::SKIN_ID_FIELD_SIGNATURE);
    b[0x300..0x300 + skin.len()].copy_from_slice(&skin);
    let report = audit_bytes(&b, Path::new("日本語 path/game.exe")).unwrap();
    let row = |name: &str| report.signatures.iter().find(|s| s.name == name).unwrap();
    assert_eq!(
        row("ITEM_MAX_DAMAGE_SIGNATURE").state,
        MatchState::Candidates
    );
    assert_eq!(row("ITEM_MAX_DAMAGE_SIGNATURE").rvas, vec![0x1020, 0x1040]);
    assert_eq!(row("SKIN_ID_FIELD_SIGNATURE").state, MatchState::Unique);
    assert_eq!(row("ACCESSOR_SIGNATURE").state, MatchState::Missing);
    assert_eq!(report.outcome, Outcome::IssuesFound);
    assert_eq!(report.outcome.exit_code(), 1);
}
#[test]
fn capped_results_are_explicit_and_not_claimed_as_an_exact_count() {
    let mut b = fixture();
    let p = literal(signatures::ITEM_MAX_DAMAGE_SIGNATURE);
    for i in 0..257 {
        let at = 0x200 + i * p.len();
        b[at..at + p.len()].copy_from_slice(&p);
    }
    let report = audit_bytes(&b, Path::new("many.exe")).unwrap();
    let row = report
        .signatures
        .iter()
        .find(|s| s.name == "ITEM_MAX_DAMAGE_SIGNATURE")
        .unwrap();
    assert_eq!(row.rvas.len(), MATCH_LIMIT);
    assert!(row.capped);
}
#[test]
fn recorded_identity_cannot_bypass_signature_or_field_validation() {
    let mut b = fixture();
    let known = &compatibility::BUILDS[1];
    put32(&mut b, 0x88, known.timestamp);
    put32(&mut b, 0x98 + 56, known.image_size);
    put32(&mut b, 0x98 + 64, known.checksum);
    let report = audit_bytes(&b, Path::new("forged-identity.exe")).unwrap();
    assert_eq!(report.recorded_build.as_deref(), Some(known.name));
    assert_eq!(report.outcome, Outcome::IssuesFound);
}
#[test]
fn report_roundtrip_and_comparison_distinguish_catalog_edits_from_binary_changes() {
    let before = audit_bytes(&fixture(), Path::new("before.exe")).unwrap();
    let mut after: Report = serde_json::from_slice(&serde_json::to_vec(&before).unwrap()).unwrap();
    assert!(compare(&before, &after).is_empty());
    after.signatures[0].rvas.push(0x1000);
    assert!(compare(&before, &after)[0].detail.contains("Matches:"));
    after.signatures[0].pattern = "90".into();
    assert!(compare(&before, &after)[0]
        .detail
        .contains("Catalog pattern changed"));
    after.signatures.remove(0);
    assert!(compare(&before, &after)
        .iter()
        .any(|c| c.detail == "Removed from catalog"));
}

#[test]
fn comparison_detects_static_field_changes_even_when_signature_hits_stay_the_same() {
    let before = audit_bytes(&fixture(), Path::new("before.exe")).unwrap();
    let mut after = before.clone();
    assert!(compare(&before, &after).is_empty());
    after.checks[0]
        .rvas_or_values
        .insert("offset".into(), 0x658);
    let changes = compare(&before, &after);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].name, after.checks[0].name);
    assert!(changes[0].detail.contains("field values changed"));
    after.checks[0].state = CheckState::Passed;
    assert!(compare(&before, &after)[0]
        .detail
        .contains("Failed -> Passed"));
    after.checks[0].state = before.checks[0].state;
    after.checks[0].rvas_or_values.clear();
    after.checks[0].detail.push_str(" (new reason)");
    assert_eq!(
        compare(&before, &after)[0].detail,
        "Static check detail changed"
    );
    after.checks.remove(0);
    after.checks.push(Check {
        name: "New camera check".into(),
        state: CheckState::Review,
        detail: "Runtime confirmation needed".into(),
        rvas_or_values: BTreeMap::new(),
    });
    let changes = compare(&before, &after);
    assert!(changes.iter().any(|c| c.detail == "Removed static check"));
    assert!(changes.iter().any(|c| c.detail == "Added static check"));
}

fn camera_fixture() -> Vec<u8> {
    let mut data = fixture();
    let position = literal(signatures::CAMERA_POSITION_COPY_SIGNATURE);
    data[0x250..0x250 + position.len()].copy_from_slice(&position);
    data[0x2ff] = 0xcc;
    data[0x300..0x30c].copy_from_slice(&[
        0x41, 0x57, 0x41, 0x56, 0x41, 0x54, 0x56, 0x57, 0x53, 0x48, 0x81, 0xec,
    ]);
    let mut render = literal(signatures::RENDER_CAMERA_ORIGIN_SIGNATURE);
    for (at, value) in [
        (14, 0x64c),
        (22, 0x654),
        (28, 0x654),
        (34, 0x660),
        (41, 0x64c),
        (48, 0x658),
    ] {
        put32(&mut render, at, value);
    }
    data[0x380..0x380 + render.len()].copy_from_slice(&render);
    data
}

#[test]
fn camera_layout_checks_validate_fields_and_bounded_function_start() {
    let report = audit_bytes(&camera_fixture(), Path::new("camera.exe")).unwrap();
    let position = report
        .checks
        .iter()
        .find(|c| c.name == "Freecam camera position layout")
        .unwrap();
    assert_eq!(position.state, CheckState::Passed);
    assert_eq!(position.rvas_or_values["position"], 0x40);
    let render = report
        .checks
        .iter()
        .find(|c| c.name == "Freecam render camera layout")
        .unwrap();
    assert_eq!(render.state, CheckState::Passed, "{}", render.detail);
    assert_eq!(render.rvas_or_values["origin"], 0x64c);
    assert_eq!(render.rvas_or_values["origin_copy"], 0x658);
    assert_eq!(render.rvas_or_values["function_rva"], 0x1100);
}

#[test]
fn a_unique_camera_pattern_does_not_hide_changed_wildcard_fields_or_missing_prologue() {
    for (at, value) in [
        (14, 0x650),
        (22, 0x658),
        (28, 0x658),
        (34, 0x664),
        (41, 0x650),
        (48, 0x680),
    ] {
        let mut data = camera_fixture();
        put32(&mut data, 0x380 + at, value);
        let report = audit_bytes(&data, Path::new("changed.exe")).unwrap();
        assert_eq!(
            report
                .signatures
                .iter()
                .find(|s| s.name == "RENDER_CAMERA_ORIGIN_SIGNATURE")
                .unwrap()
                .state,
            MatchState::Unique
        );
        assert_eq!(
            report
                .checks
                .iter()
                .find(|c| c.name == "Freecam render camera layout")
                .unwrap()
                .state,
            CheckState::Failed,
            "field at {at}"
        );
    }
    for at in [0x2ff, 0x300] {
        let mut data = camera_fixture();
        data[at] = 0x90;
        let report = audit_bytes(&data, Path::new("no-prologue.exe")).unwrap();
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "Freecam render camera layout")
            .unwrap();
        assert_eq!(check.state, CheckState::Failed);
        assert!(check.detail.contains("prologue"));
    }
}
#[test]
fn export_never_overwrites_input_baseline_or_existing_files() {
    let report = audit_bytes(&fixture(), Path::new("source.json")).unwrap();
    let dir = std::env::temp_dir().join(format!(
        "mio-signature-export-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("existing.json");
    std::fs::write(&path, b"original").unwrap();
    assert!(save_report(&path, &report).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert!(save_report(&dir.join("game.exe"), &report).is_err());
    let new = dir.join("新規.json");
    save_report(&new, &report).unwrap();
    let restored: Report = serde_json::from_slice(&std::fs::read(&new).unwrap()).unwrap();
    assert_eq!(restored.schema_version, 1);
    std::fs::remove_file(new).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
#[test]
fn cli_parser_handles_quoted_paths_and_rejects_incomplete_or_duplicate_flags() {
    let parsed = cli::parse(
        [
            "--exe",
            "D:/日本語 path/game.exe",
            "--baseline",
            "before.json",
            "--out",
            "after.json",
        ]
        .map(OsString::from),
    )
    .unwrap();
    assert_eq!(parsed.exe, Some(PathBuf::from("D:/日本語 path/game.exe")));
    for args in [
        vec!["--exe"],
        vec!["--unknown"],
        vec!["--exe", "--out", "after.json"],
        vec!["--exe", "a.exe", "--exe", "b.exe"],
    ] {
        assert!(cli::parse(args.into_iter().map(OsString::from)).is_err());
    }
}
use std::ffi::OsString;

#[test]
fn both_local_analyzed_builds_pass_with_current_catalog_and_camera_cross_checks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap();
    for (path, name) in [
        (PathBuf::from(DEFAULT_EXE), "1.26.65.1"),
        (
            root.join(
                "Microsoft.MinecraftUWP_1.26.6301.0_x64__8wekyb3d8bbwe/Minecraft.Windows.exe",
            ),
            "1.26.6301.0",
        ),
    ] {
        if !path.is_file() {
            continue;
        }
        let mut data = std::fs::read(&path).unwrap();
        // Only run this fixture assertion when the installed file is still the
        // recorded build. Future updates must be judged, not fail unit tests.
        let pe = pe::Pe::parse(&data).unwrap();
        if !compatibility::BUILDS.iter().any(|b| {
            b.name == name
                && (b.timestamp, b.image_size, b.checksum)
                    == (
                        pe.identity.timestamp,
                        pe.identity.image_size,
                        pe.identity.checksum,
                    )
        }) {
            continue;
        }
        let report = audit_bytes(&data, &path).unwrap();
        assert_eq!(report.signatures.len(), signatures::CATALOG.len());
        assert_eq!(report.signatures.len(), SIGNATURE_COUNT);
        assert_eq!(report.recorded_build.as_deref(), Some(name));
        assert_eq!(
            report.outcome,
            Outcome::StaticChecksPassed,
            "{}",
            summary(&report)
        );
        assert!(report.checks.iter().all(|c| c.state == CheckState::Passed));
        for name in [
            "Freecam camera position layout",
            "Freecam render camera layout",
        ] {
            assert!(report.checks.iter().any(|c| c.name == name));
        }
        // A future header identity with the same anchors still needs ABI review.
        let pe_offset = pe::u32_at(&data, 0x3c).unwrap() as usize;
        let timestamp = pe::u32_at(&data, pe_offset + 8).unwrap();
        put32(&mut data, pe_offset + 8, timestamp.wrapping_add(1));
        let updated = audit_bytes(&data, &path).unwrap();
        assert_eq!(updated.recorded_build, None);
        assert_eq!(updated.outcome, Outcome::ReviewRequired);
        assert_eq!(updated.outcome.exit_code(), 2);
    }
}

#[test]
fn derived_getter_patterns_are_found_among_covering_catalog_hits_like_a_full_scan() {
    let mut b = fixture();
    let shape = literal(signatures::GAMMA_GETTER_SHAPE);
    for (raw, id) in [(0x220, 7u32), (0x300, 9), (0x400, 7), (0x500, 0x1234)] {
        b[raw..raw + shape.len()].copy_from_slice(&shape);
        put32(&mut b, raw + signatures::GAMMA_ID_AT, id);
    }
    let source = scan(pe::Pe::parse(&b).unwrap()).unwrap();
    use compatibility::Code;
    for id in [7, 9, 0x1234, 8, 0] {
        let pattern = signatures::gamma_signature(id);
        let parsed: sigscan::Pattern = pattern.parse().unwrap();
        assert!(
            source.find_covered(&parsed).is_some(),
            "covered by the shape"
        );
        assert_eq!(source.find(&pattern), source.find_by_scan(&parsed), "{id}");
    }
    assert_eq!(
        source.find(&signatures::gamma_signature(7)),
        [0x1020, 0x1200]
    );
    // A pattern no catalog entry covers is still scanned for.
    let other: sigscan::Pattern = "CC CC CC".parse().unwrap();
    assert!(source.find_covered(&other).is_none());
}

#[cfg(windows)]
#[test]
fn mapped_file_is_audited_like_a_copy_and_cannot_be_written_while_open() {
    let dir = std::env::temp_dir().join(format!(
        "mio-signature-map-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("game.exe");
    let data = fixture();
    std::fs::write(&path, &data).unwrap();
    {
        let view = mapped::Mapped::open(&path, 1 << 20)
            .unwrap()
            .expect("mapped");
        assert_eq!(view.bytes(), &data[..]);
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(mapped::Mapped::open(&path, 16).is_err(), "size limit");
    }
    {
        // Someone writing: no view (the copying reader takes over).
        let _writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert!(mapped::Mapped::open(&path, 1 << 20).unwrap().is_none());
    }
    let from_file = audit_file(&path, None).unwrap();
    let from_bytes = audit_bytes(&data, &path.canonicalize().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&from_file.signatures).unwrap(),
        serde_json::to_value(&from_bytes.signatures).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&from_file.checks).unwrap(),
        serde_json::to_value(&from_bytes.checks).unwrap()
    );
    let empty = dir.join("empty.exe");
    std::fs::write(&empty, b"").unwrap();
    assert!(mapped::Mapped::open(&empty, 1 << 20).unwrap().is_none());
    assert!(audit_file(&empty, None).is_err());
    // The view is unmapped in the background after the audit.
    for _ in 0..100 {
        if std::fs::remove_file(&path).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::fs::remove_file(&empty).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}
