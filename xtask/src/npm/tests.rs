use super::*;

#[test]
fn a_release_goes_to_latest_and_a_candidate_to_next() {
    assert_eq!(dist_tag("0.2.0"), "latest");
    assert_eq!(dist_tag("0.2.0-rc.1"), "next");
}

#[test]
fn trusted_publishing_needs_npm_11_5_1() {
    assert!(npm_can_publish("11.5.1") && npm_can_publish("11.10.0"));
    assert!(!npm_can_publish("11.5.0") && !npm_can_publish("10.9.2"));
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
        "loaded": SMOKE_MODELS,
        "ioStarted": true,
        "state": {"listening": "listening", "recognising": 0, "playback": "idle", "online": true},
        "webAudioIo": "function",
    });
    assert_eq!(check_smoke(&ran), Ok(()));
    for (key, value) in [
        ("loaded", json!(["smoke-vad"])),
        ("ioStarted", json!(false)),
        ("state", json!(null)),
        ("webAudioIo", json!("undefined")),
    ] {
        let mut report = ran.clone();
        report[key] = value;
        assert!(check_smoke(&report).is_err(), "{key}");
    }
}
