use super::*;

#[test]
fn a_release_goes_to_latest_and_a_candidate_to_next() {
    assert_eq!(dist_tag("0.2.0"), "latest");
    assert_eq!(dist_tag("0.2.0-rc.1"), "next");
}

#[test]
fn the_stage_id_is_read_from_npm_s_report() {
    let report = r#"{"@sidevoice/voice": {"id": "@sidevoice/voice@0.1.0", "stageId": "abc-123"}}"#;
    assert_eq!(stage_id(report).as_deref(), Some("abc-123"));
    assert_eq!(stage_id("not json"), None);
}

#[test]
fn a_staged_version_is_found_unless_rejected_or_approved() {
    let list = r#"[
        {"id": "s-1", "packageName": "@sidevoice/voice", "version": "0.1.0", "status": "rejected", "shasum": "aa"},
        {"id": "s-2", "packageName": "@sidevoice/engine", "version": "0.1.0", "status": "pending", "shasum": "bb"},
        {"id": "s-3", "packageName": "@sidevoice/voice", "version": "0.1.0", "status": "pending", "shasum": "cc"}
    ]"#;
    let found = staged(list, "0.1.0").unwrap();
    assert_eq!((found.id.as_str(), found.shasum.as_str()), ("s-3", "cc"));
    assert_eq!(staged(list, "0.2.0"), None);
    assert_eq!(staged("[]", "0.1.0"), None);
    assert_eq!(staged("not json", "0.1.0"), None);
}

/// What npm answered on 2026-10-09 when @sidevoice/voice@0.0.0 was published again while staged.
const STAGED_CONFLICT: &str = "npm error code E409\nnpm error 409 Conflict - PUT https://registry.npmjs.org/@sidevoice%2fvoice - \
                               Cannot publish over previously staged version \"0.0.0\".";

#[test]
fn npm_s_staged_conflict_means_the_version_is_staged_already() {
    assert!(already_staged(STAGED_CONFLICT));
    assert!(!already_staged(
        "npm error code E409\nnpm error 409 Conflict - PUT https://registry.npmjs.org/x - busy"
    ));
    assert!(!already_staged(
        "npm error code E403\nnpm error 403 Forbidden - previously staged"
    ));
}

#[test]
fn the_shasum_is_npm_s_sha1() {
    assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
}

#[test]
fn a_package_needs_the_wasm_build_and_every_js_file() {
    let js = ["index.js".to_owned(), "index.d.ts".to_owned()];
    let full = [
        "dist/sidevoice_voice.js",
        "dist/sidevoice_voice.d.ts",
        "dist/sidevoice_voice_bg.wasm",
        "js/index.js",
        "js/index.d.ts",
        "package.json",
    ];
    assert!(missing(&full, &js).is_empty());
    assert_eq!(
        missing(&full[1..4], &js),
        ["dist/sidevoice_voice.js", "js/index.d.ts"]
    );
}

#[test]
fn the_shipped_js_is_what_the_package_names() {
    let manifest = parse(
        &read(&repo().join("npm/package.json")).unwrap(),
        "package.json",
    )
    .unwrap();
    let js = js_files(&repo().join("js")).unwrap();
    let exports = &manifest["exports"]["."];
    for entry in [&exports["default"], &exports["types"], &manifest["types"]] {
        let file = entry.as_str().unwrap().strip_prefix("./js/").unwrap();
        assert!(js.iter().any(|name| name == file), "{file} is not in js/");
    }
}

#[test]
fn a_smoke_report_must_show_a_running_call() {
    let ran = json!({
        "loaded": SMOKE_SLOTS,
        "ioStarted": true,
        "listenedEarly": false,
        "said": {"id": "string", "steps": ["playing", "progress", "progress", "done"], "outcome": {"status": "heard"}},
        "state": {"listening": "listening", "recognising": 0, "playback": "idle", "online": true},
        "webAudioIo": "function",
        "host": {
            "missing": "settings-missing", "unknown": "model-unknown", "reloaded": 1,
            "smartMissing": "end-of-turn-missing", "catalogue": 2, "flags": ["muted"],
            "early": "not-played", "said": ["function", "heard"],
            "started": "resolved", "again": "resolved", "restarted": "resolved", "afterRestart": "listening",
            "smartLive": ["listening", []],
            "states": ["idle", "listening", "idle"],
            "keys": [true, "sk-smoke", false], "seam": SEAM,
        },
    });
    assert_eq!(check_smoke(&ran), Ok(()));
    for (key, value) in [
        ("loaded", json!(["vad"])),
        ("ioStarted", json!(false)),
        ("listenedEarly", json!(true)),
        (
            "said",
            json!({"id": "string", "steps": ["done"], "outcome": {"status": "not-played", "reason": "stopped"}}),
        ),
        ("state", json!(null)),
        ("webAudioIo", json!("undefined")),
        ("host", json!(null)),
    ] {
        let mut report = ran.clone();
        report[key] = value;
        assert!(check_smoke(&report).is_err(), "{key}");
    }
    for (key, value) in [
        ("started", json!("stopped")),
        ("states", json!(["idle", "listening"])),
        ("keys", json!([false, null, false])),
        ("seam", json!(["start"])),
        ("reloaded", json!(0)),
        ("flags", json!(["listening"])),
        ("said", json!(["function", "not-played"])),
        ("afterRestart", json!("idle")),
        ("smartLive", json!(["idle", ["end-of-turn-missing"]])),
    ] {
        let mut report = ran.clone();
        report["host"][key] = value;
        assert!(check_smoke(&report).is_err(), "host {key}");
    }
}

#[test]
fn a_staged_version_this_job_cannot_compare_is_a_failure_naming_the_shasum_to_check() {
    // `publish` returns this as its error: an unverifiable conflict never passes for success.
    let failed = staging_failed(STAGED_CONFLICT, "@sidevoice/voice@0.0.0", "abc123", "wf");
    assert!(failed.contains("could not compare"), "{failed}");
    assert!(
        failed.contains("approve it only if its shasum is abc123"),
        "{failed}"
    );
    let other = staging_failed(
        "npm error code E403",
        "@sidevoice/voice@0.0.0",
        "abc123",
        "wf",
    );
    assert!(
        other.contains("trusted publishing") && other.contains("(wf)"),
        "{other}"
    );
    assert!(!other.contains("abc123"));
}
