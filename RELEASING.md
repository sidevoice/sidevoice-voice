# Releasing

One version for the crate, tagged `vX.Y.Z`. It lives in `Cargo.toml` (and `Cargo.lock`); release-please moves it
(`release-please-config.json`: release type `rust`, `bump-minor-pre-major`, draft releases). Never edit it by hand.

The call reaches its consumers in two ways:

- **Native consumers** (the desktop app) depend on the crate at a git tag and compile it themselves: source, no
  binaries.
- **The web** gets `@sidevoice/voice` on npm: the wasm32 build of the crate as `wasm-bindgen --target web` emits it,
  the browser's microphone and speaker and the entry point (`js/`), with its `package.json`. `@sidevoice/engine` is a
  peer dependency: the page brings the engine. Published for `vX.Y.Z` releases only ([npm](#npm)).

## What each act means

| Act | Who | What happens |
|---|---|---|
| Open / update a PR | anyone | `ci`: format and Clippy (native, wasm32 and xtask), the native tests on macOS and Linux, the whole call with real models, the wasm32 tests in Node, and the npm package built and installed as a consumer installs it (`cargo xtask npm`, `npm-smoke`), publishing nothing; the package is kept 7 days as the artifact `voice-npm-<head sha>` ([A pull request's package](#a-pull-requests-package)). **PR title is a conventional commit**. |
| Squash-merge into `main` | reviewer | The PR title becomes the commit. `release` runs: the wasm32 tests, the npm package built and smoke-tested; then it attests the assets, attaches them to the **`nightly`** pre-release, reads them back, verifies them and publishes it. Never on npm. release-please opens or updates the **release PR** ("chore(main): release X.Y.Z"). |
| Merge the release PR | a maintainer | **This is the release.** release-please tags `vX.Y.Z` and creates a draft GitHub Release whose notes are that version's changelog; `release` runs from the tag, attaches and verifies the assets, and publishes the Release; then it publishes `@sidevoice/voice@X.Y.Z` to npm. |

Everything besides the GitHub steps is either plain `cargo` or code in `xtask/` (`cargo xtask npm | npm-smoke |
manifest | publish | npm-publish`, each described at the top of `xtask/src/main.rs`: thin calls to `cargo`,
`wasm-bindgen`, `npm`, `node` and `gh`), so it runs the same on a laptop:

- `cargo test --locked --target wasm32-unknown-unknown --lib` runs the module's tests compiled to wasm32 in Node:
  `.cargo/config.toml` makes `wasm-bindgen-test-runner` the runner for that target.
- `cargo xtask npm` builds the crate for wasm32 in release mode, runs `wasm-bindgen --target web` on it into
  `dist/`, copies `js/`, adds the checked-in `npm/package.json` (its version stamped from the crate's), `npm/README.md`
  and the licence, and packs the package with `npm pack` into `target/npm/sidevoice-voice-X.Y.Z.tgz`. It fails if
  the tarball lacks wasm-bindgen's entry point, types or wasm, or any file of `js/`.
- `cargo xtask npm-smoke` installs that tarball into a scratch project (`target/npm smoke/`) with `npm install`, as
  a consumer does (without the peer: the test brings a fake engine), and in Node imports `@sidevoice/voice`, loads
  its wasm from `node_modules` and runs `VoiceCall.create` on a plain-object engine and a plain-object microphone and
  speaker: the call must load its three models through the engine, in order, start the microphone and speaker, and
  say it listens.

The wasm32 tests and `cargo xtask npm` need the wasm-bindgen CLI (`wasm-bindgen`, `wasm-bindgen-test-runner`) on the
`PATH`, at the version of the `wasm-bindgen` crate in `Cargo.lock`. In CI, `.github/actions/setup` installs it with
`taiki-e/install-action`, which checks the release binary against the SHA-256 it pins; bump that version with the
crate's. npm is the one on the `PATH`; publishing needs 11.5.1 or later (the release workflow sets up Node.js 24).

A native consumer pins a release by its tag:

```toml
sidevoice-voice = { git = "https://github.com/sidevoice/sidevoice-voice", tag = "vX.Y.Z" }
```

## Assets

Every GitHub Release (and the nightly) carries:

- `sidevoice-voice-X.Y.Z.tgz`: the npm package exactly as `npm publish` sends it (on the nightly,
  `sidevoice-voice-nightly.tgz`, a fixed name whose download URL never changes; the version inside is the crate's).
- `SHA256SUMS`.
- `attestation.sigstore.json`: one SLSA provenance attestation whose subject is the tarball.

The signer is the workflow `release.yml` on `main`, for nightlies and releases alike. Verify an asset with:

```sh
gh attestation verify sidevoice-voice-0.1.0.tgz \
  --repo sidevoice/sidevoice-voice \
  --bundle attestation.sigstore.json \
  --cert-identity 'https://github.com/sidevoice/sidevoice-voice/.github/workflows/release.yml@refs/heads/main' \
  --deny-self-hosted-runners
```

The changelog is written from the squashed PR titles. To change it, edit `CHANGELOG.md` in the release PR right
before merging it: any later merge into `main` regenerates the PR. After the release, fix the notes on the
Release itself.

## npm

Every `vX.Y.Z` release (never the nightly) is published to npm as `@sidevoice/voice` at that version: dist-tag
`latest`, or `next` for a version with a `-` suffix (a release candidate).

Only the release workflow publishes, by npm's **trusted publishing** (OIDC) with provenance: no npm token exists.
The job `publish-npm` ("Publish to npm") in `release.yml` runs after the GitHub Release is published, for versioned
releases only, as one step, `cargo xtask npm-publish vX.Y.Z`:

1. It downloads the Release's assets and checks `sidevoice-voice-X.Y.Z.tgz` against `SHA256SUMS` and the
   attestation (`gh attestation verify`, signer `release.yml` on `main`, GitHub-hosted runner): npm gets the bytes
   GitHub Releases has, nothing rebuilt.
2. It publishes that tarball with `npm publish --access public --provenance --tag latest|next` (npm 11.5.1 or later;
   it fails clearly with an older one). A version already published with the same bytes (compared with what
   `npm pack @sidevoice/voice@X.Y.Z` fetches from the registry) is skipped, so a re-run carries on; with other bytes
   it fails: **npm versions are immutable** (a published version can never be replaced, only deprecated), so a bad
   release is fixed by the next version.

### Before the first versioned release (the operator, once)

This is the operator's job, done once on npmjs.com before the first release PR is merged; no workflow and no
contributor can do it. Until it is done the `publish-npm` job fails and nothing reaches npm (the GitHub Release is
published regardless):

1. **The scope and the name.** `@sidevoice/voice` lives in the `@sidevoice` organisation, which must allow its
   members to create public packages. A trusted publisher is configured on the package's settings page, which only
   exists once the package does: publish it once by hand, from an account in the org, as a public `0.0.0`
   placeholder (`npm publish --access public`), so the name is ours.
2. **The trusted publisher.** `@sidevoice/voice` → Settings → Trusted publisher → GitHub Actions: organisation
   `sidevoice`, repository `sidevoice-voice`, no environment, and the workflow filename npm checks. **npm checks the
   workflow that starts the run, not one it calls**: a version is released by `release-please.yml`, which calls
   `release.yml` (`workflow_call`), so the filename to enter is **`release-please.yml`**. The job's error names the
   filename it saw when it does not match.
3. **No tokens.** In the package's access settings, require 2FA and disallow tokens: only trusted publishing
   remains.

Every workflow on the way grants `id-token: write`, and the job sets up Node.js 24, which brings npm 11: trusted
publishing needs 11.5.1 or later, and `cargo xtask npm-publish` checks it.

## Which version comes next

`fix:` → patch, `feat:` → minor. While the version is 0.x a breaking change (`feat!:` or a `BREAKING CHANGE:`
footer) bumps the minor, not the major. `docs:`, `chore:`, `ci:`, `test:`, `refactor:` alone make no release.

## A release candidate, or any explicit version

Put the footer as the **last line of a PR's description** (the squash commit takes the description as its body):

```
Release-As: 0.1.0-rc.1
```

The release PR then proposes exactly that version. A version with a `-` suffix is published as a **pre-release and
never as latest**, on GitHub and on npm (`next`). The next candidate is `Release-As: 0.1.0-rc.2`; the final one is
`Release-As: 0.1.0` (say it: after a candidate, do not leave the next version to the computation). With nothing
else to merge, a PR with one empty commit (`git commit --allow-empty`) carries the footer.

## Nightly

Every green `release` run on `main` moves the tag `nightly` to that commit and replaces every asset of the one
`nightly` pre-release. Its notes give the commit. It is a snapshot, not a version: never latest, never on npm, and
release-please ignores the tag. Pin a `vX.Y.Z` release, never `nightly`.

Build artifacts on Actions runs are kept 7 days, for debugging and for trying a commit before it is released (below).
To depend on the call, download from Releases.

## A pull request's package

Every pull request's `ci` run uploads the npm package it built as the Actions artifact **`voice-npm-<sha>`**, `<sha>`
being the PR's head commit (all 40 characters), and every push to `main` does the same for that commit (`release`).
Each is kept **7 days**; after that, push again or re-run the job. It is built, as everything in `ci`, from GitHub's
merge of that head into the PR's base, and published nowhere: a snapshot to try, never a dependency.

From a PR to its package:

```sh
sha=$(gh pr view 4 --repo sidevoice/sidevoice-voice --json headRefOid --jq .headRefOid)
gh api "repos/sidevoice/sidevoice-voice/actions/artifacts?name=voice-npm-$sha" \
  --jq '[.artifacts[] | select(.expired | not)][0].archive_download_url'
```

That URL needs a GitHub token (any, read access; GitHub asks for one even on a public repository) and answers a zip
holding the one `sidevoice-voice-X.Y.Z.tgz`, which `npm install` takes as it is. While the run is still going, the
artifact is not there yet; when the job that builds it failed, it is never there.

## When something fails

- A release build or its verification fails: the Release stays a draft, its tag in place. Fix forward if needed,
  then re-run the failed jobs of that `release-please` run. Nothing is published until every check passed.
- A `nightly` run fails: the previous snapshot stays. The next green push replaces it.
- The npm job fails: the GitHub Release is already published and stays. Fix the cause (usually the trusted
  publisher settings: the error names them) and re-run that job; a version already published with the same bytes
  is skipped.
- A release run is never cancelled half-way; nightlies queue behind each other.

## What this needs from the repository settings

- Settings → Actions → General → **Allow GitHub Actions to create and approve pull requests**: without it
  release-please cannot open its PR.
- Squash merging, with the PR title as the commit message.
- `ci` and **PR title is a conventional commit** run on every PR; release-please's own PR gets both through a
  dispatched run (its pushes start no workflow by themselves). Make both required in a ruleset to enforce them.
- On npmjs.com, the scope and the trusted publisher of `@sidevoice/voice`, the operator's once ([Before the first
  versioned release](#before-the-first-versioned-release-the-operator-once)).
