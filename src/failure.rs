//! The failures this module logs, as stable codes. A line about a failure carries `error.type = Failure::X.code()`
//! and the controlled facts around it (a duration, a device error's kind), never an error's `Display` or `Debug`.
//! The codes the app's models and devices hand over as strings are the people-facing codes of `VoiceError`; they are
//! logged beside this one, as `reason`, once checked against their grammar (`redact::code`).

#[cfg(test)]
mod tests;

/// What went wrong, by where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    /// A model did not load.
    ModelLoad,
    /// A transcription did not finish.
    Transcription,
    /// A reply could not be spoken.
    Synthesis,
    /// The microphone went away or could not open.
    InputLost,
    /// A failure was handed to the app.
    Reported,
    /// An audio stream stopped calling back.
    #[cfg(native)]
    AudioStreamStalled,
    /// The system took an audio stream away.
    #[cfg(native)]
    AudioStreamTaken,
    /// An audio stream reported an error.
    #[cfg(native)]
    AudioStreamError,
    /// An audio stream could not be opened again.
    #[cfg(native)]
    AudioStreamReopen,
    /// The iOS audio session refused a step.
    #[allow(dead_code, reason = "only the iOS platform module raises it")]
    AudioSession,
}

impl Failure {
    /// Every code, for the test that holds them unique and stable.
    #[cfg(test)]
    const ALL: &'static [Failure] = &[
        Self::ModelLoad,
        Self::Transcription,
        Self::Synthesis,
        Self::InputLost,
        Self::Reported,
        #[cfg(native)]
        Self::AudioStreamStalled,
        #[cfg(native)]
        Self::AudioStreamTaken,
        #[cfg(native)]
        Self::AudioStreamError,
        #[cfg(native)]
        Self::AudioStreamReopen,
        Self::AudioSession,
    ];

    /// The stable code, `voice.<kebab-name>`: the `error.type` of a log line.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::ModelLoad => "voice.model-load-failed",
            Self::Transcription => "voice.transcription-failed",
            Self::Synthesis => "voice.synthesis-failed",
            Self::InputLost => "voice.input-lost",
            Self::Reported => "voice.failure-reported",
            #[cfg(native)]
            Self::AudioStreamStalled => "voice.audio-stream-stalled",
            #[cfg(native)]
            Self::AudioStreamTaken => "voice.audio-stream-taken",
            #[cfg(native)]
            Self::AudioStreamError => "voice.audio-stream-error",
            #[cfg(native)]
            Self::AudioStreamReopen => "voice.audio-stream-reopen-failed",
            Self::AudioSession => "voice.audio-session-refused",
        }
    }
}
