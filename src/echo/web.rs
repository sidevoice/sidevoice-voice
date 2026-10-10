//! In the browser the browser cancels the echo: the page's microphone asks `getUserMedia` for `echoCancellation`
//! (with `noiseSuppression` and `autoGainControl`), and the browser takes its reference from what it plays. No
//! canceller is linked into the wasm32 build, so the audio is never processed twice.
#![cfg(web)]
