use objc2_avf_audio::{
    AVAudioSession, AVAudioSessionCategoryOptions, AVAudioSessionCategoryPlayAndRecord,
    AVAudioSessionModeDefault, AVAudioSessionModeVoiceChat, AVAudioSessionSetActiveOptions,
};
use objc2_foundation::NSError;

use super::{AudioSession, SessionConfig, SessionMode};

/// iOS: `AVAudioSession.sharedInstance()`, the process's one session. The category is play-and-record, whatever the
/// mode and the options. cpal's Core Audio host sets no category and no mode of its own (it only asks for a buffer
/// size and reopens after an interruption), so what is configured here is what the system applies.
///
/// The system, not the program, picks the output route, so the speaker is not selectable.
pub struct Os;

impl AudioSession for Os {
    const OUTPUT_SELECTABLE: bool = false;

    fn configure(config: &SessionConfig) -> Result<(), String> {
        let mut options = AVAudioSessionCategoryOptions::empty();
        if config.speaker {
            options |= AVAudioSessionCategoryOptions::DefaultToSpeaker;
        }
        if config.bluetooth {
            options |= AVAudioSessionCategoryOptions::AllowBluetoothHFP;
        }
        // SAFETY: the constants are Apple's, linked from AVFAudio, and the session is the process's singleton.
        let (category, mode) = unsafe {
            let mode = match config.mode {
                SessionMode::Default => AVAudioSessionModeDefault,
                SessionMode::VoiceChat => AVAudioSessionModeVoiceChat,
            };
            (AVAudioSessionCategoryPlayAndRecord, mode)
        };
        let (Some(category), Some(mode)) = (category, mode) else {
            return Err(failed("configure", None));
        };
        // SAFETY: a message to the shared session with arguments of the types it takes.
        unsafe {
            AVAudioSession::sharedInstance().setCategory_mode_options_error(category, mode, options)
        }
        .map_err(|error| failed("configure", Some(&*error)))
    }

    fn activate() -> Result<(), String> {
        // SAFETY: a message to the shared session.
        unsafe { AVAudioSession::sharedInstance().setActive_error(true) }
            .map_err(|error| failed("activate", Some(&*error)))
    }

    fn deactivate() -> Result<(), String> {
        // SAFETY: a message to the shared session.
        unsafe {
            AVAudioSession::sharedInstance().setActive_withOptions_error(
                false,
                AVAudioSessionSetActiveOptions::NotifyOthersOnDeactivation,
            )
        }
        .map_err(|error| failed("deactivate", Some(&*error)))
    }
}

/// The stable code of the step that failed; the system's own error (its domain and code, such as `561015905` for
/// recording that cannot start from the background) goes to the log.
fn failed(step: &str, error: Option<&NSError>) -> String {
    match error {
        Some(error) => tracing::warn!(
            step,
            code = error.code(),
            description = %error.localizedDescription(),
            "the audio session refused"
        ),
        None => tracing::warn!(step, "the audio session's category or mode is missing"),
    }
    format!("audio-session-{step}-failed")
}
