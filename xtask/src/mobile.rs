//! `cargo xtask mobile-build ios|android`: the crate built for the phones' targets, linked but not run.
//!
//! - `ios`: `aarch64-apple-ios` (devices) and `aarch64-apple-ios-sim` (the simulator on Apple silicon), in release,
//!   on a Mac with Xcode: the iOS SDK is what links them.
//! - `android`: `arm64-v8a` (phones) and `x86_64` (the emulator) with cargo-ndk, in release, at API level 26, the
//!   oldest cpal's AAudio host supports. Needs the NDK (`ANDROID_NDK_HOME`) and the two Rust targets.
//!
//! A build here is what a phone's app compiles in; running the call on a device is the app's gate. The crate's cdylib
//! is linked too (`crate-type`), so a symbol the target lacks fails the build.

use std::env;
use std::process::Command;

use crate::{repo, Result};

/// The Android ABIs built, and the API level they are linked for.
const ABIS: [&str; 2] = ["arm64-v8a", "x86_64"];
const API_LEVEL: &str = "26";
/// The iOS targets built.
const IOS_TARGETS: [&str; 2] = ["aarch64-apple-ios", "aarch64-apple-ios-sim"];

/// `cargo xtask mobile-build ios|android`.
pub(crate) fn build(platform: &str) -> Result<()> {
    let mut args: Vec<&str> = Vec::new();
    match platform {
        "ios" => {
            args.push("build");
            args.extend(["--locked", "--release"]);
            for target in IOS_TARGETS {
                args.extend(["--target", target]);
            }
        }
        "android" => {
            args.extend(["ndk", "-P", API_LEVEL]);
            for abi in ABIS {
                args.extend(["-t", abi]);
            }
            args.extend(["build", "--locked", "--release"]);
        }
        other => return Err(format!("mobile-build: `{other}` is not ios or android")),
    }
    let status = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(&args)
        .current_dir(repo())
        .status()
        .map_err(|e| format!("cargo {}: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {}: {status}", args.join(" ")))
    }
}
