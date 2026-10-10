//! Echo cancellation: the call's own playback is taken out of what the microphone hears before the detector sees it.
//! Natively that is WebRTC's AEC3, fed with the playback as the speaker takes it (`native`); in the browser it is the
//! browser's own canceller, which the page's microphone asks for (`web`). Each file says where it runs.

mod native;
mod web;

#[cfg(native)]
pub(crate) use native::EchoCanceller;
