//! The call on npm: `@sidevoice/voice`, the wasm32 build as `wasm-bindgen --target web` emits it (`dist/`), the
//! JavaScript microphone and speaker and the entry point (`js/`), the checked-in `npm/package.json` (its version
//! stamped from the crate's) and `npm/README.md`, and the licence.

use std::path::{Path, PathBuf};
use std::{env, fs};

use serde_json::{json, Value};

use crate::release::{download_verified, tarball_name};
use crate::{empty_dir, metadata, read, repo, run_in, sh, write, Result};

#[cfg(test)]
mod tests;

const PACKAGE: &str = "@sidevoice/voice";
/// The library name, after which wasm-bindgen names its output.
const STEM: &str = "sidevoice_voice";
/// The npm CLI `npm-publish` runs: staged publishing needs `npm stage`, trusted publishing 11.5.1 or later. Bumped by
/// hand, like any pin.
const NPM_VERSION: &str = "11.21.0";
const SMOKE_JS: &str = include_str!("../npm/smoke.mjs");
/// The models the smoke test's configuration names, in the order a call loads its stages: vad, stt, tts.
const SMOKE_MODELS: [&str; 3] = ["smoke-vad", "smoke-stt", "smoke-tts"];
/// The calls of the voice seam, `VoiceHost` (js/voice-host.d.ts), sorted.
const SEAM: [&str; 16] = [
    "cancelInput",
    "hasProviderKey",
    "models",
    "mute",
    "onError",
    "onKaraoke",
    "onLevel",
    "onPlayback",
    "onState",
    "onUserTurn",
    "setOnline",
    "setProviderKey",
    "setSettings",
    "speak",
    "start",
    "stop",
];

fn parse(bytes: &[u8], what: &str) -> Result<Value> {
    serde_json::from_slice(bytes).map_err(|error| format!("{what}: {error}"))
}

/// `cargo xtask npm`: build, bind and pack into `target/npm/sidevoice-voice-X.Y.Z.tgz`.
pub(crate) fn package() -> Result<()> {
    let (version, target) = metadata()?;
    sh("cargo build --locked --release --lib --target wasm32-unknown-unknown")?;
    let pkg = target.join("npm-package");
    empty_dir(&pkg)?;
    let wasm = format!("wasm32-unknown-unknown/release/{STEM}.wasm");
    let bindgen = format!("wasm-bindgen --target web --out-dir npm-package/dist {wasm}");
    run_in(&target, &bindgen, &[])?;

    let mut manifest = parse(&read(&repo().join("npm/package.json"))?, "npm/package.json")?;
    manifest["version"] = version.clone().into();
    let manifest = format!("{manifest:#}\n");
    write(&pkg.join("package.json"), manifest.as_bytes())?;
    for (from, to) in [("npm/README.md", "README.md"), ("LICENSE", "LICENSE")] {
        fs::copy(repo().join(from), pkg.join(to)).map_err(|error| format!("{from}: {error}"))?;
    }
    copy_dir(&repo().join("js"), &pkg.join("js"))?;

    empty_dir(&target.join("npm"))?;
    let report = run_in(&pkg, "npm pack --json --pack-destination ../npm", &[])?;
    let report = &parse(report.as_bytes(), "npm pack")?[0];
    let packed: Vec<&str> = report["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| file["path"].as_str())
        .collect();
    let js = js_files(&repo().join("js"))?;
    let left_out = missing(&packed, &js);
    if !left_out.is_empty() {
        return Err(format!("npm pack left out {}", left_out.join(", ")));
    }
    let tarball = tarball_name(&version);
    if report["filename"] != tarball.as_str() {
        let wrote = &report["filename"];
        return Err(format!("npm pack wrote {wrote}, not {tarball}"));
    }
    println!("{}", target.join("npm").join(tarball).display());
    Ok(())
}

/// What a packed package must hold and `packed` lacks: wasm-bindgen's entry point, types and wasm, and every file of
/// `js/` (named in `js`).
fn missing(packed: &[&str], js: &[String]) -> Vec<String> {
    let dist = [".js", ".d.ts", "_bg.wasm"].map(|suffix| format!("dist/{STEM}{suffix}"));
    let js = js.iter().map(|file| format!("js/{file}"));
    dist.into_iter()
        .chain(js)
        .filter(|file| !packed.contains(&file.as_str()))
        .collect()
}

/// The names of the files in `dir`, sorted.
fn js_files(dir: &Path) -> Result<Vec<String>> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into())
        .collect();
    names.sort();
    Ok(names)
}

/// Copies the files of `from` (not its subdirectories: `js/` has none) into a new directory `to`.
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    empty_dir(to)?;
    for name in js_files(from)? {
        fs::copy(from.join(&name), to.join(&name))
            .map_err(|error| format!("{}: {error}", from.join(&name).display()))?;
    }
    Ok(())
}

/// `cargo xtask npm-smoke`: install the tarball into a consumer's project and, in Node, run a call on a fake engine
/// and a fake microphone and speaker.
pub(crate) fn smoke() -> Result<()> {
    let (version, target) = metadata()?;
    // Beside target/npm, in a path with a space in it, as a home directory may have.
    let dir = target.join("npm smoke");
    empty_dir(&dir)?;
    write(&dir.join("package.json"), br#"{"type": "module"}"#)?;
    let tarball = format!("../npm/{}", tarball_name(&version));
    // The engine is a peer the consumer brings; the smoke test brings a fake one, so npm installs no peer.
    let install = "npm install --no-audit --no-fund --legacy-peer-deps";
    run_in(&dir, install, &[&tarball])?;
    write(&dir.join("smoke.mjs"), SMOKE_JS.as_bytes())?;
    let wasm = format!("../dist/{STEM}_bg.wasm");
    let models = SMOKE_MODELS.join(",");
    let report = run_in(&dir, "node smoke.mjs", &[&wasm, &models])?;
    let report = parse(report.as_bytes(), "smoke.mjs")?;
    check_smoke(&report)?;
    println!("{PACKAGE}@{version} installs and runs: {report}");
    Ok(())
}

/// Whether the smoke test's report shows a call that ran: it loaded every stage through the engine, in order, started
/// the microphone and speaker, and told its state as listening; the package offers the default web microphone and
/// speaker; and its voice seam answers as `VoiceHost` says.
fn check_smoke(report: &Value) -> Result<()> {
    let (loaded, want) = (&report["loaded"], json!(SMOKE_MODELS));
    if *loaded != want {
        return Err(format!("the call loaded {loaded}, not {want}"));
    }
    if report["ioStarted"] != true {
        return Err(format!("the call never started its io: {report}"));
    }
    if report["state"]["listening"] != "listening" {
        return Err(format!("the call told no listening state: {report}"));
    }
    if report["webAudioIo"] != "function" {
        return Err(format!("the package has no createWebAudioIo: {report}"));
    }
    let host = &report["host"];
    let want = json!({
        "missing": "settings-missing", "unknown": "model-unknown", "wrongTask": "model-wrong-task",
        "buildUnfit": "build-unfit", "smartTurn": "end-of-turn-unavailable", "reloaded": 3,
        "started": "resolved", "again": "resolved", "states": host["states"], "keys": [true, "sk-smoke", false],
        "seam": SEAM,
    });
    if *host != want {
        return Err(format!("the voice seam answered {host}, not {want}"));
    }
    let states: Vec<&str> = host["states"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !states.contains(&"listening") || states.last() != Some(&"idle") {
        return Err(format!(
            "the voice seam's states were {states:?}: never listening, or not idle after stop"
        ));
    }
    Ok(())
}

/// The dist-tag of a version: `next` for one with a `-` suffix (a release candidate), else `latest`.
fn dist_tag(version: &str) -> &'static str {
    if version.contains('-') {
        "next"
    } else {
        "latest"
    }
}

/// `cargo xtask npm-publish TAG`: the Release's tarball, verified, **staged** on npm by trusted publishing with
/// provenance (`npm stage publish`): the organisation publishes nothing directly, and a maintainer approves each staged
/// version on npmjs.com with 2FA. A version already published, or already staged, with these very bytes is left as it
/// is, so a re-run carries on.
pub(crate) fn publish(tag: &str) -> Result<()> {
    let version = tag.strip_prefix('v').unwrap_or_default();
    if !version.starts_with(char::is_numeric) {
        return Err(format!("{tag}: not a vX.Y.Z; the nightly is never on npm"));
    }
    let npm = npm_cli()?;

    // Exactly the bytes the GitHub Release holds, checked against its SHA256SUMS and attestation.
    let dir: PathBuf = env::temp_dir().join("sidevoice-voice-npm-publish");
    empty_dir(&dir)?;
    let name = tarball_name(version);
    if !download_verified(tag, &dir)?.contains(&name) {
        return Err(format!("{name}: not in the Release's SHA256SUMS"));
    }
    let tarball = read(&dir.join(&name))?;
    let shasum = sha1_hex(&tarball);

    let spec = format!("{PACKAGE}@{version}");
    let registry = dir.join("registry");
    empty_dir(&registry)?;
    match npm.run(&registry, &["pack", &spec]) {
        Err(error) if error.contains("E404") || error.contains("ETARGET") => {}
        Err(error) => return Err(error),
        Ok(_) if read(&registry.join(&name))? == tarball => {
            println!("{spec} is already published with these bytes; nothing to approve");
            return Ok(());
        }
        Ok(_) => {
            return Err(format!(
                "{spec} is on npm with other bytes: npm versions are immutable, release anew"
            ))
        }
    }
    // Staged already (by an earlier run of this release): the same bytes wait for approval; other bytes must be
    // rejected first. Listing staged versions may need a login this job does not have: then staging says.
    match npm.run(&dir, &["stage", "list", PACKAGE, "--json"]) {
        Ok(list) => match staged(&list, version) {
            Some(item) if item.shasum == shasum => {
                println!("{spec} is already STAGED with these bytes. {}", approval(&spec, &item.id));
                return Ok(());
            }
            Some(item) => {
                return Err(format!(
                    "{spec} is staged with other bytes (stage id {}, shasum {}; this build {shasum}): reject it on \
                     npmjs.com or with `npm stage reject {}`, then re-run",
                    item.id, item.shasum, item.id
                ))
            }
            None => {}
        },
        Err(error) => println!("could not list staged versions, staging anyway: {error}"),
    }
    let dist_tag = dist_tag(version);
    let args = [
        "stage",
        "publish",
        &name,
        "--access",
        "public",
        "--provenance",
        "--tag",
        dist_tag,
        "--json",
    ];
    let report = match npm.run(&dir, &args) {
        Err(error) if already_staged(&error) => {
            println!(
                "{spec} is already staged (npm refused to stage it again). {}",
                approval(&spec, &format!("(see `npm stage list {PACKAGE}`)"))
            );
            return Ok(());
        }
        result => result,
    };
    let report = report.map_err(|error| {
        let workflow = env::var("GITHUB_WORKFLOW_REF").unwrap_or_default();
        format!(
            "{error}\nnpm takes trusted publishing only: on npmjs.com, {PACKAGE} → Settings → Trusted publisher \
             must name this repository and the workflow that started this run ({workflow}). If {spec} is already \
             staged, approve or reject it on npmjs.com."
        )
    })?;
    let id = stage_id(&report).unwrap_or_else(|| format!("(see `npm stage list {PACKAGE}`)"));
    println!(
        "{spec} is STAGED (dist-tag {dist_tag}). {}",
        approval(&spec, &id)
    );
    Ok(())
}

/// Whether npm refused to stage a version because that version is staged already: the registry answers `E409` with
/// "Cannot publish over previously staged version" (`STAGED_CONFLICT` in the tests, as npm said it). The job cannot list
/// staged versions itself (trusted publishing authenticates the publish only), so this is how a re-run of a release
/// finds the version it staged before.
fn already_staged(error: &str) -> bool {
    error.contains("E409") && error.to_ascii_lowercase().contains("previously staged")
}

/// What a maintainer does with a staged version.
fn approval(spec: &str, id: &str) -> String {
    format!(
        "Stage id {id}, awaiting a maintainer's approval with 2FA: npmjs.com → {PACKAGE} → staged versions, or \
         `npm stage approve {id}`. Until then {spec} is not installable; `npm stage reject {id}` drops it."
    )
}

/// A staged version, as `npm stage list --json` lists it.
#[derive(Debug, PartialEq)]
struct Staged {
    id: String,
    shasum: String,
}

/// The staged, not rejected, `version` of the package in `npm stage list --json`'s report.
fn staged(report: &str, version: &str) -> Option<Staged> {
    let items: Value = serde_json::from_str(report).ok()?;
    items.as_array()?.iter().find_map(|item| {
        let ours = item["packageName"] == PACKAGE && item["version"] == version;
        let live = !matches!(item["status"].as_str(), Some("rejected" | "approved"));
        (ours && live).then(|| Staged {
            id: item["id"].as_str().unwrap_or_default().to_owned(),
            shasum: item["shasum"].as_str().unwrap_or_default().to_owned(),
        })
    })
}

/// The stage id in `npm stage publish --json`'s report.
fn stage_id(report: &str) -> Option<String> {
    fn find(value: &Value) -> Option<String> {
        match value {
            Value::Object(map) => map
                .get("stageId")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| map.values().find_map(find)),
            Value::Array(items) => items.iter().find_map(find),
            _ => None,
        }
    }
    find(&serde_json::from_str(report).ok()?)
}

/// The SHA-1 of a tarball, as npm's `shasum` names it.
fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::Digest;
    sha1::Sha1::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The npm CLI the publishing step runs: `NPM_VERSION`, installed under `target/npm-cli` (staged publishing needs
/// `npm stage`, trusted publishing 11.5.1 or later), run by the Node.js on the `PATH`.
struct NpmCli {
    script: PathBuf,
}

impl NpmCli {
    fn run(&self, dir: &Path, args: &[&str]) -> Result<String> {
        let script = self.script.to_str().ok_or("npm-cli.js: not a UTF-8 path")?;
        let mut all = vec![script];
        all.extend_from_slice(args);
        run_in(dir, "node", &all)
    }
}

fn npm_cli() -> Result<NpmCli> {
    let (_, target) = metadata()?;
    let prefix = target.join("npm-cli");
    let script = prefix.join("node_modules/npm/bin/npm-cli.js");
    if !script.exists() {
        fs::create_dir_all(&prefix).map_err(|error| format!("{}: {error}", prefix.display()))?;
        let spec = format!("npm@{NPM_VERSION}");
        run_in(
            &prefix,
            "npm install --no-audit --no-fund --prefix .",
            &[&spec],
        )?;
    }
    let cli = NpmCli { script };
    let version = cli.run(&prefix, &["--version"])?;
    if version.trim() != NPM_VERSION {
        return Err(format!("npm {}: expected {NPM_VERSION}", version.trim()));
    }
    Ok(cli)
}
