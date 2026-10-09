//! Build tooling for sidevoice-voice, run as `cargo xtask <command>` (alias in `.cargo/config.toml`): thin calls to
//! `cargo`, `wasm-bindgen`, `npm`, `node` and `gh` from the `PATH`. The wasm32 tests need no xtask: `cargo test
//! --target wasm32-unknown-unknown --lib` (its runner is set in `.cargo/config.toml`).
//!
//! - `npm`: the npm package `@sidevoice/voice` packed into `target/npm/` (xtask/src/npm.rs).
//! - `npm-smoke`: that tarball installed as a consumer installs it, and a call run in Node on a fake engine and a
//!   fake microphone and speaker.
//! - `manifest DIR [--tag vX.Y.Z]`: the npm tarball in DIR renamed as published (`sidevoice-voice-nightly.tgz` for
//!   the nightly) and `SHA256SUMS` written over DIR (xtask/src/release.rs).
//! - `publish DIR TAG`: DIR attached to the GitHub Release TAG, read back, verified, and the Release published.
//! - `npm-publish TAG`: the tarball of the Release TAG (a `vX.Y.Z`), verified, published to npm.

mod npm;
mod release;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs};

use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, String>;

const USAGE: &str =
    "usage: cargo xtask npm | npm-smoke | manifest DIR [--tag vX.Y.Z] | publish DIR TAG | npm-publish TAG";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["npm"] => npm::package(),
        ["npm-smoke"] => npm::smoke(),
        ["manifest", dir] => release::manifest(Path::new(dir), None),
        ["manifest", dir, "--tag", tag] => release::manifest(Path::new(dir), Some(tag)),
        ["publish", dir, tag] => release::publish(Path::new(dir), tag),
        ["npm-publish", tag] => npm::publish(tag),
        _ => Err(USAGE.into()),
    };
    if let Err(error) = &result {
        eprintln!("xtask: {error}");
    }
    ExitCode::from(u8::from(result.is_err()))
}

/// The repository root: xtask/ is a package of its own beside the crate.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Runs the command `words` (split at spaces; `cargo` is the running Cargo) in the repository.
fn sh(words: &str) -> Result<String> {
    run_in(&repo(), words, &[])
}

/// Runs the command `words` (split at spaces), then `args` as they are, in `dir`, and returns its standard output;
/// a failure is an error carrying its standard error.
fn run_in(dir: &Path, words: &str, args: &[&str]) -> Result<String> {
    let mut words = words.split_whitespace();
    let program = match words.next() {
        Some("cargo") => env::var("CARGO").unwrap_or_else(|_| "cargo".into()),
        program => program.unwrap_or_default().to_string(),
    };
    let args: Vec<&str> = words.chain(args.iter().copied()).collect();
    let what = format!("{program} {}", args.join(" "));
    let out = Command::new(&program).args(&args).current_dir(dir).output();
    let out = out.map_err(|error| format!("{what}: {error}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{what}: {}\n{stderr}", out.status));
    }
    String::from_utf8(out.stdout).map_err(|_| format!("{what}: output is not UTF-8"))
}

/// The crate's version (the release's) and Cargo's target directory, from `cargo metadata`.
fn metadata() -> Result<(String, PathBuf)> {
    let meta = sh("cargo metadata --locked --no-deps --format-version 1")?;
    let meta: serde_json::Value = serde_json::from_str(&meta).map_err(|e| e.to_string())?;
    let packages = meta["packages"].as_array().into_iter().flatten();
    let voice = packages.filter(|package| package["name"] == "sidevoice-voice");
    let version = voice
        .filter_map(|package| package["version"].as_str())
        .next();
    let target = meta["target_directory"].as_str();
    match (version, target) {
        (Some(version), Some(target)) => Ok((version.into(), target.into())),
        _ => Err("cargo metadata: no sidevoice-voice package or target directory".into()),
    }
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// Removes `dir` if it is there and creates it empty.
fn empty_dir(dir: &Path) -> Result<()> {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
