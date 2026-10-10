//! What the call is asked to say (`VoiceCall::say`), and what becomes of it: a handle ([`Saying`]) under the call's own
//! id for it, which can be cancelled and tells, in order, when it starts to sound, where the reader is, and how it
//! ended ([`SayOutcome`]).

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_util::Stream;
use serde::Serialize;

use crate::voice_call::Message;

/// How to say it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SayOptions {
    /// The language the text is written in, a BCP 47 tag; the configuration's when absent.
    pub language: Option<String>,
}

/// One step of something being said. `Done` is the last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SayEvent {
    /// Its first sound reached the speaker.
    Playing,
    /// Where the reader is, in characters (Unicode scalar values) of the text: the chunk sounding now (from its first
    /// character to the one after its last; `None` between chunks), and what was heard from the start (the chunks
    /// played to their end).
    Progress {
        sounding: Option<(usize, usize)>,
        heard_chars: usize,
    },
    /// How it ended.
    Done { outcome: SayOutcome },
}

/// How something said ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum SayOutcome {
    /// Heard to its end.
    Heard,
    /// It sounded, and was cut: heard up to character `heard_chars` (at a chunk boundary), for `reason`.
    HeardUpTo {
        heard_chars: usize,
        reason: StopReason,
    },
    /// It never sounded, for `reason`.
    NotPlayed { reason: StopReason },
}

/// Why something said stopped short, or never sounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum StopReason {
    /// Its handle was cancelled.
    Cancelled,
    /// The person started a turn over it, or before its turn came.
    BargeIn,
    /// The call stopped, or was stopped when it was asked.
    Stopped,
    /// It could not be spoken: the stable code of why (`speech-failed`, ...), also told as an error.
    Failed { code: String },
}

/// Something the call is saying. Its events come in order and end with [`SayEvent::Done`]; dropping the handle does not
/// cancel it.
#[derive(Debug)]
pub struct Saying {
    id: String,
    cancel: SayCancel,
    events: UnboundedReceiver<SayEvent>,
}

/// Cancels something being said, from anywhere: the part not yet heard is dropped, and its outcome says `cancelled`.
#[derive(Debug, Clone)]
pub struct SayCancel {
    id: String,
    messages: UnboundedSender<Message>,
}

impl Saying {
    pub(crate) fn new(
        id: String,
        messages: UnboundedSender<Message>,
        events: UnboundedReceiver<SayEvent>,
    ) -> Self {
        Self {
            cancel: SayCancel {
                id: id.clone(),
                messages,
            },
            id,
            events,
        }
    }

    /// The call's id for it.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Cancels it.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// What cancels it, kept apart from its events.
    #[must_use]
    pub fn canceller(&self) -> SayCancel {
        self.cancel.clone()
    }
}

impl SayCancel {
    /// Cancels it; once it has ended, nothing.
    pub fn cancel(&self) {
        let _ = self
            .messages
            .unbounded_send(Message::CancelSay(self.id.clone()));
    }
}

impl Stream for Saying {
    type Item = SayEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<SayEvent>> {
        Pin::new(&mut self.events).poll_next(cx)
    }
}
