//! What a call runs ([`VoiceSetup`]): its input half ([`VoiceInput`]: the microphone, heard through the app's
//! [`InputModels`]), its output half ([`VoiceOutput`]: the speaker, spoken through the app's [`OutputModels`]), each
//! optional, and what holds for the whole call. A half that is absent is neither loaded nor opened: a call without an
//! input half opens no microphone and loads no detector, transcriber or end-of-turn model; one without an output half
//! loads no speaker model and plays nothing.

use std::sync::Arc;

use crate::config::{CallConfig, InputConfig, OutputConfig, VoiceConfig};
use crate::models::{InputModels, OutputModels};

/// The halves of a call and its configuration.
#[derive(Clone, Default)]
pub struct VoiceSetup {
    /// Listening: absent, no microphone, no detector, no transcriber, no end-of-turn model.
    pub input: Option<VoiceInput>,
    /// Speaking: absent, no speaker model and no playback; what the call is asked to say is not played
    /// (`failed`, `output-missing`).
    pub output: Option<VoiceOutput>,
    pub config: VoiceConfig,
}

/// The input half: where its models come from, and how it listens.
#[derive(Clone)]
pub struct VoiceInput {
    pub models: Arc<dyn InputModels>,
    pub config: InputConfig,
}

/// The output half: where its model comes from, and how it speaks.
#[derive(Clone)]
pub struct VoiceOutput {
    pub models: Arc<dyn OutputModels>,
    pub config: OutputConfig,
}

impl VoiceSetup {
    /// The configuration the call's state machine runs on.
    pub(crate) fn call_config(&self) -> CallConfig {
        CallConfig {
            input: self.input.as_ref().map(|input| input.config.clone()),
            output: self.output.as_ref().map(|output| output.config.clone()),
            call: self.config.clone(),
        }
    }
}

impl std::fmt::Debug for VoiceSetup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VoiceSetup")
            .field("input", &self.input.as_ref().map(|input| &input.config))
            .field("output", &self.output.as_ref().map(|output| &output.config))
            .field("config", &self.config)
            .finish()
    }
}
