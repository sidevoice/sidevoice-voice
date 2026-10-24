//! The optional native C++/Sonora comparison and its metric checks, on the current host.
use crate::{sh, Result};

pub(super) fn run() -> Result<()> {
    println!("Commit: `{}`\n", sh("git rev-parse HEAD")?.trim());
    println!("Host: `{}`\n", sh("uname -a")?.trim());
    println!("Rust: `{}`\n", sh("rustc --version")?.trim());
    println!(
        "C++: `{}`\n",
        sh("c++ --version")?.lines().next().unwrap_or_default()
    );
    let manifest = "--manifest-path tools/audio-comparison/Cargo.toml";
    sh(&format!("cargo fmt {manifest} --check"))?;
    sh(&format!(
        "cargo clippy --locked --release {manifest} --lib --bin audio-comparison -- -D warnings"
    ))?;
    let tests = sh(&format!("cargo test --locked --release {manifest} --lib"))?;
    eprintln!("{tests}");
    let output = sh(&format!(
        "cargo run --locked --release {manifest} --bin audio-comparison"
    ))?;
    println!("{output}");
    Ok(())
}
