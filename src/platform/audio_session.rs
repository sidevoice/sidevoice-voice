//! The operating system's audio session: how this process uses audio, and what the system says of the output route.
//!
//! Some systems (iOS) keep a session for the process: its **category** (it plays and records), its **mode** and its
//! **options** are declared to the system, and the session is **activated** for the process to have the audio
//! hardware. These are separate steps on purpose. [`AudioSession::configure`] only declares, and can be done at any
//! time and again whenever the declaration changes; [`AudioSession::activate`] is when the system applies it, and can
//! come later, when the system hands the process its audio. Where the system keeps no such session, all three do
//! nothing and succeed.

#[cfg(test)]
mod tests;

/// How the process uses the microphone and the speaker, as the system should know it.
///
/// A call plays and records, so the category is always play-and-record; what varies is declared here. There is no
/// default: which mode and options a call wants is a decision of whoever starts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionConfig {
    /// The processing the system applies to the call's audio.
    pub mode: SessionMode,
    /// Whether the output goes to the loudspeaker rather than the receiver (the earpiece) when no other route, such as
    /// a headset, has been chosen.
    pub speaker: bool,
    /// Whether a Bluetooth hands-free device may be the microphone and the speaker.
    pub bluetooth: bool,
}

/// The processing the system applies to a session's audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    /// None beyond the category's.
    Default,
    /// The system's tuning for a voice call: a level and a routing it chooses for speech. It brings no echo
    /// cancellation to the audio unit the call opens.
    VoiceChat,
}

/// What the engine asks of the operating system about audio sessions. An OS says what it can do; where it has no
/// session, it implements every method as a call that does nothing and succeeds.
pub trait AudioSession {
    /// Whether the speaker can be chosen among the outputs the system lists. False where the system picks the route
    /// (iOS: the receiver, the loudspeaker, a headset or a Bluetooth device, changed in the system's own controls).
    const OUTPUT_SELECTABLE: bool;

    /// Declares how the process uses audio. Does not activate.
    ///
    /// # Errors
    ///
    /// `audio-session-configure-failed` when the system refuses the declaration.
    fn configure(config: &SessionConfig) -> Result<(), String>;

    /// Has the system apply the declaration and give the process the audio hardware.
    ///
    /// # Errors
    ///
    /// `audio-session-activate-failed` when the system refuses (another process holds the audio, a call is in
    /// progress, recording cannot start from the background).
    fn activate() -> Result<(), String>;

    /// Gives the audio hardware back, telling the processes whose audio this one interrupted that they can go on.
    ///
    /// # Errors
    ///
    /// `audio-session-deactivate-failed` when the system refuses (audio is still running).
    fn deactivate() -> Result<(), String>;
}

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::Os as Session;

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
pub use ios::Os as Session;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Os as Session;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::Os as Session;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Os as Session;
