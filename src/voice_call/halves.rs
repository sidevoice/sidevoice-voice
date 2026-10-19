//! The call's halves in its task: where each gets its models, what it loaded, and a change of it that waits for its
//! safe point (the input's: no turn open; the output's: nothing being said) while its models load.

use std::sync::Arc;

use futures_util::future::join3;

use crate::config::{InputConfig, OutputConfig};
use crate::models::{EndOfTurnModel, InputModels, OutputModels, Speaker, Transcriber, Vad};

/// The input half: where its models come from, and those it loaded.
pub(super) struct InputHalf {
    pub(super) models: Arc<dyn InputModels>,
    pub(super) loaded: Option<LoadedInput>,
}

/// The input half's models, loaded.
pub(super) struct LoadedInput {
    pub(super) vad: Box<dyn Vad>,
    pub(super) transcriber: Arc<dyn Transcriber>,
    /// Loaded only for `smart`.
    pub(super) end_of_turn: Option<Arc<dyn EndOfTurnModel>>,
}

/// The output half: where its model comes from, and the speaker it loaded.
pub(super) struct OutputHalf {
    pub(super) models: Arc<dyn OutputModels>,
    pub(super) speaker: Option<Arc<dyn Speaker>>,
}

/// What an input half's load gave: each slot asked for, if it loaded, and the first failure.
#[derive(Default)]
pub(crate) struct InputSlots {
    vad: Option<Box<dyn Vad>>,
    transcriber: Option<Arc<dyn Transcriber>>,
    end_of_turn: Option<Arc<dyn EndOfTurnModel>>,
    pub(super) failure: Option<String>,
}

impl InputSlots {
    /// Whether it holds a detector and a transcriber: a whole input half.
    pub(super) fn whole(&self) -> bool {
        self.vad.is_some() && self.transcriber.is_some()
    }
}

/// How much silence warms a detector up, in 16 kHz samples: 0.1 s, a whole window of any common detector.
pub(super) const WARM_UP_SAMPLES: usize = 1_600;

/// A fresh detector, warmed up: its first windows (a runtime's first inference is the slow one) run on silence, and it
/// starts over from sample 0, with no speech.
async fn warm(models: &dyn InputModels) -> Result<Box<dyn Vad>, String> {
    let mut vad = models.vad().await?;
    vad.accept(&[0.0; WARM_UP_SAMPLES]).await?;
    vad.reset().await;
    Ok(vad)
}

/// Loads, all at once, the detector and the transcriber (`whole`) and the end-of-turn model (`end_of_turn`). The
/// detector is warmed up as it loads, while the others still do.
pub(super) async fn load_input(
    models: Arc<dyn InputModels>,
    whole: bool,
    end_of_turn: bool,
) -> InputSlots {
    let detector = async {
        match whole {
            true => Some(warm(models.as_ref()).await),
            false => None,
        }
    };
    let transcriber = async {
        match whole {
            true => Some(models.transcriber().await),
            false => None,
        }
    };
    let classifier = async {
        match end_of_turn {
            true => Some(models.end_of_turn().await),
            false => None,
        }
    };
    let (vad, transcriber, end_of_turn) = join3(detector, transcriber, classifier).await;
    let mut slots = InputSlots::default();
    let mut fail = |code: String| {
        slots.failure.get_or_insert(code);
    };
    let vad = vad.transpose().map_err(&mut fail).ok().flatten();
    let transcriber = transcriber.transpose().map_err(&mut fail).ok().flatten();
    let end_of_turn = end_of_turn.transpose().map_err(&mut fail).ok().flatten();
    slots.vad = vad;
    slots.transcriber = transcriber;
    slots.end_of_turn = end_of_turn;
    slots
}

impl InputHalf {
    pub(super) fn new(models: Arc<dyn InputModels>) -> Self {
        Self {
            models,
            loaded: None,
        }
    }

    /// Takes what a load gave: a detector with a transcriber replace what it loaded; an end-of-turn model joins it.
    /// Without `smart` it keeps no end-of-turn model.
    pub(super) fn take(&mut self, slots: InputSlots, smart: bool) {
        if let (Some(vad), Some(transcriber)) = (slots.vad, slots.transcriber) {
            self.loaded = Some(LoadedInput {
                vad,
                transcriber,
                end_of_turn: None,
            });
        }
        if let Some(loaded) = &mut self.loaded {
            if smart {
                loaded.end_of_turn = slots.end_of_turn.or(loaded.end_of_turn.take());
            } else {
                loaded.end_of_turn = None;
            }
        }
    }

    /// What it still has to load for its configuration: the detector and the transcriber, and the end-of-turn model.
    pub(super) fn missing(&self, smart: bool) -> (bool, bool) {
        let whole = self.loaded.is_none();
        let end_of_turn = smart
            && self
                .loaded
                .as_ref()
                .is_none_or(|loaded| loaded.end_of_turn.is_none());
        (whole, end_of_turn)
    }
}

impl OutputHalf {
    pub(super) fn new(models: Arc<dyn OutputModels>) -> Self {
        Self {
            models,
            speaker: None,
        }
    }
}

/// A change of the input half waiting for its safe point.
pub(super) struct PendingInput {
    /// The half it becomes, with its configuration; `None`: it goes.
    pub(super) target: Option<(Arc<dyn InputModels>, InputConfig)>,
    /// What loaded for it so far.
    pub(super) slots: InputSlots,
    /// Whether what it needs has loaded (or it needs nothing).
    pub(super) ready: bool,
    /// Which change this is: a load that answers for an older one is dropped.
    pub(super) serial: u64,
}

/// A change of the output half waiting for its safe point.
pub(super) struct PendingOutput {
    pub(super) target: Option<(Arc<dyn OutputModels>, OutputConfig)>,
    pub(super) speaker: Option<Arc<dyn Speaker>>,
    pub(super) ready: bool,
    pub(super) serial: u64,
}
