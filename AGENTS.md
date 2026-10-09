# AGENTS.md

Rules for any coding agent (and person) working in this repository.

## Language of the code and of the product

- Code, identifiers, comments, commit messages and docs are in **English**.
- **What the module tells people carries keys.** A failure a person will read (a transcription that failed, a
  microphone that went away, a reply that could not be spoken) is said as a stable code with its parameters, never as
  a finished sentence. The app translates by code; the module's English text for each code is the fallback and is
  never parsed. No other language, Spanish included, is ever hard-coded, and nothing here picks a language for the
  person.
- Logs and developer-facing errors are English; they are not UI.

## Before changing things

Read `README.md` (what the module is and where things are). The repository is one Rust crate (`src/`), the
JavaScript the npm package ships beside its wasm build (`js/`: ES modules, no bundler, no dependencies), and the build
tooling `cargo xtask` (`xtask/`). The design is sidevoice-core#89.

- **The module holds no socket.** What it reports to the room and what the room sends it are serde messages; the
  app carries them, keeps the outbox and the acknowledgements. Nothing here knows the room's address.
- **The call is a pure state machine.** It is driven by events and a monotonic time argument, and answers with what
  to do: no I/O, no clock, no threads in it. Capture, playback, the app's models and the room are around it, and
  the tests drive it with recorded audio and fakes.
- **The models are the app's.** Voice activity, speech to text, text to speech and end of turn reach the call
  through the module's own interfaces (`src/models.rs`), which the app implements (with sidevoice-engine, say). The
  module names no model, links no model runtime and depends on no engine; which model fills each slot, and its keys,
  are the app's.
- **No compatibility code.** There are no users yet: a change replaces what it changes, without fallbacks, migrations
  or support for older messages.
- **Everything is closed by default.** Each item gets the narrowest visibility that works: private, then
  `pub(super)` or `pub(crate)`, and `pub` only for what consumers of the crate actually need. Opening something
  later is cheap, closing it later is a breaking change.

## Code layout

- **One package per concept.** `x.rs` is the module and holds its main type or trait (never `x/x.rs`: Clippy's
  `module_inception`); `x/` holds its parts, one small file per piece.
- **A file is one concept**, with its closely related types. Split by cohesion and size, never one type per file.
- **Names say what a thing is**, by role, never by state.
- **Unit tests live in `x/tests.rs`** beside their module (`#[cfg(test)] mod tests;`), never inline. Test doubles
  shared between modules are in `src/test_support.rs`, compiled in test builds only.
- **Platform conditions use the crate's aliases** (`web`, `native`, from `build.rs`), never `target_arch` directly.
  A crate-level module's condition goes on its declaration (`#[cfg(web)] mod web;`); a file that only compiles on
  one platform starts with its own `#![cfg(alias)]`, with a comment saying why, and its dependency sits under the
  same platform in `Cargo.toml`. There are no crate features and no central list of platform files.
- **Comments describe their own unit**: what this file, type or function is and does, never who calls it or what
  the rest of the system does with it.
- **What is data stays data**, and is validated strictly when read: an unknown or a missing key is an error.

## How work lands

- Pull request titles are [Conventional Commits](https://www.conventionalcommits.org) (CI checks them); a PR is
  squash-merged and its title becomes the commit, from which release notes are written.
- Commits are signed.
- Logic lives in `cargo xtask`, not in workflow YAML: a workflow sets up the machine and calls one command, which runs
  the same on a laptop.
- Third-party actions are pinned by commit SHA, with the version in a comment.
