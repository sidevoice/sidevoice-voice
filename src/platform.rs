//! The only place the native build touches the operating system. Everything else is host-agnostic.
//!
//! One module per concern: `audio_session` (how the process uses the system's audio). Each declares its trait and
//! re-exports the type of the OS being compiled, which lives in `<concern>/<os>.rs`.

pub mod audio_session;
