//! The GitHub Release: `SHA256SUMS` over its assets (`manifest`), attaching, reading back and verifying them, and
//! publishing it (`publish`). The attestation (`attestation.sigstore.json`) is made by the workflow in between.

use std::env;
use std::fs;
use std::path::Path;

use crate::{empty_dir, metadata, read, run_in, sh, sha256, write, Result};

#[cfg(test)]
mod tests;

const SUMS: &str = "SHA256SUMS";

/// The npm tarball's name: as `npm pack` names it for a version, `sidevoice-voice-nightly.tgz` for the nightly (a
/// fixed name, so its download URL never changes).
pub(crate) fn tarball_name(version: &str) -> String {
    format!("sidevoice-voice-{version}.tgz")
}

fn var(name: &str) -> Result<String> {
    env::var(name).map_err(|_| format!("{name} is not set"))
}

/// The names of the files in `dir`, sorted.
fn assets(dir: &Path) -> Result<Vec<String>> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let names = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into());
    let mut names: Vec<String> = names.collect();
    names.sort();
    Ok(names)
}

/// `cargo xtask manifest DIR [--tag vX.Y.Z]`
pub(crate) fn manifest(dir: &Path, tag: Option<&str>) -> Result<()> {
    if let Ok(expected) = env::var("GITHUB_SHA") {
        // The attestation names GITHUB_SHA as its source: the assets must come from that very commit.
        let head = sh("git rev-parse HEAD")?;
        let head = head.trim();
        if head != expected {
            return Err(format!("HEAD is {head}, but this run is for {expected}"));
        }
    }
    let (version, _) = metadata()?;
    let built = tarball_name(&version);
    match tag {
        Some(tag) if tag != format!("v{version}") => {
            return Err(format!("Cargo.toml says {version}, the release is {tag}"));
        }
        Some(_) => {}
        None => fs::rename(dir.join(&built), dir.join(tarball_name("nightly")))
            .map_err(|error| format!("{built}: {error}"))?,
    }
    print!("{}", write_sums(dir)?);
    Ok(())
}

/// Writes `SHA256SUMS` over every file in `dir` (an earlier `SHA256SUMS` is rewritten, not listed); returns it.
fn write_sums(dir: &Path) -> Result<String> {
    let mut sums = String::new();
    for name in assets(dir)?.iter().filter(|name| *name != SUMS) {
        sums += &format!("{}  {name}\n", sha256(&read(&dir.join(name))?));
    }
    write(&dir.join(SUMS), sums.as_bytes())?;
    Ok(sums)
}

/// Lines of a `SHA256SUMS` file as (digest, name).
fn parse_sums(text: &str) -> Result<Vec<(&str, &str)>> {
    let line = |line| str::split_once(line, "  ").ok_or(format!("malformed {SUMS} line: {line}"));
    text.lines().map(line).collect()
}

/// Downloads every asset of the Release `tag` into `dir` and checks each one `SHA256SUMS` lists against its digest
/// and the attestation, whose signer must be `release.yml` on `main`, on a GitHub-hosted runner. Returns their names.
pub(crate) fn download_verified(tag: &str, dir: &Path) -> Result<Vec<String>> {
    run_in(dir, &format!("gh release download {tag}"), &[])?;
    let repo = var("GH_REPO")?;
    let signer = format!("https://github.com/{repo}/.github/workflows/release.yml@refs/heads/main");
    let verify = format!("gh attestation verify --repo {repo} --cert-identity {signer}");
    let mut checked = Vec::new();
    let sums = String::from_utf8_lossy(&read(&dir.join(SUMS))?).into_owned();
    for (digest, name) in parse_sums(&sums)? {
        if sha256(&read(&dir.join(name))?) != digest {
            return Err(format!("{name}: not the bytes {SUMS} lists"));
        }
        let args = [
            "--deny-self-hosted-runners",
            "--bundle",
            "attestation.sigstore.json",
            name,
        ];
        run_in(dir, &verify, &args)
            .map_err(|error| format!("{name}: attestation check failed: {error}"))?;
        checked.push(name.to_string());
    }
    Ok(checked)
}

/// `cargo xtask publish DIR TAG`: attach every file in DIR to the Release TAG (for `nightly`, move the tag to
/// GITHUB_SHA first and drop older assets; a `vX.Y.Z` is release-please's draft), read them back, verify, publish.
pub(crate) fn publish(dir: &Path, tag: &str) -> Result<()> {
    let names = assets(dir)?;
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    if tag == "nightly" {
        let sha = var("GITHUB_SHA")?;
        let refs = format!("gh api repos/{}/git/refs", var("GH_REPO")?);
        let patch = format!("{refs}/tags/nightly -X PATCH -F force=true -f sha={sha}");
        let post = format!("{refs} -X POST -f ref=refs/tags/nightly -f sha={sha}");
        if sh(&patch).is_err() {
            sh(&post)?;
        }
        if sh("gh release view nightly").is_err() {
            sh("gh release create nightly --verify-tag --draft --notes -")?;
        }
        let notes = format!(
            "Snapshot of `main` at {sha}. Not a version: the `nightly` tag moves to every commit on `main` whose \
             build passes, and these assets are replaced each time. Never on npm. Pin a `vX.Y.Z` release instead."
        );
        let about = ["--title", "Nightly (main)", "--notes", &notes];
        let edit = "gh release edit nightly --prerelease --latest=false";
        run_in(dir, edit, &about)?;
        run_in(dir, "gh release upload nightly --clobber", &names)?;
        // Anything left from an older snapshot that this one did not replace.
        for old in sh("gh release view nightly --json assets -q .assets[].name")?.lines() {
            if !names.contains(&old) {
                sh(&format!("gh release delete-asset nightly {old} --yes"))?;
            }
        }
    } else {
        run_in(dir, &format!("gh release upload {tag} --clobber"), &names)?;
    }

    // Read every asset back: listed in SHA256SUMS, the bytes of this build, signed by the release workflow.
    let check = env::temp_dir().join("sidevoice-voice-release-check");
    empty_dir(&check)?;
    for name in download_verified(tag, &check)? {
        if read(&check.join(&name))? != read(&dir.join(&name))? {
            return Err(format!("{name}: the Release holds other bytes than these"));
        }
    }
    let latest = !(tag == "nightly" || tag.contains('-'));
    let state = format!("--prerelease={} --latest={latest}", !latest);
    sh(&format!("gh release edit {tag} --draft=false {state}"))?;
    println!("published {tag}");
    Ok(())
}
