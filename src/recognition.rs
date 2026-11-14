//! The recognition queue: turns wait here for their transcript, one job at a time and in order, at most eight turns. A
//! long turn comes as several jobs, its pieces in order, the last one when it ended. A transcript passes the acceptance
//! filter (`filter`), then waits out the merge window: a turn that follows within it is joined to it, so a sentence
//! split by a pause is reported once.

mod filter;
mod join;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::fmt;

use crate::redact::chars;

pub(crate) use filter::accepted;
pub(crate) use join::join;

/// The most turns waiting for, or in, recognition, whatever the number of their pieces.
pub(crate) const MAX_QUEUE: usize = 8;

/// A turn's audio, or a piece of it, waiting for its transcript.
#[derive(Debug, PartialEq)]
pub(crate) struct Job {
    pub(crate) turn: usize,
    pub(crate) pcm: Vec<f32>,
    /// Whether it is the turn's last piece: the turn ended with it.
    pub(crate) last: bool,
    /// The language it was recorded in, which the configuration may have left by the time its transcript arrives.
    pub(crate) language: Option<String>,
}

/// A transcript held for the merge window.
#[derive(PartialEq)]
pub(crate) struct Pending {
    /// The turns it holds, oldest first; the last one carries the transcript.
    pub(crate) turns: Vec<usize>,
    pub(crate) text: String,
    /// The language it was recorded in: only transcripts of one language are joined.
    language: Option<String>,
    deadline: u64,
}

/// The queue, the job in recognition, and the transcript held for the merge window.
#[derive(Debug, Default)]
pub(crate) struct Recognition {
    queue: VecDeque<Job>,
    /// The turn of the job in recognition, and whether it is that turn's last piece.
    active: Option<(usize, bool)>,
    pending: Option<Pending>,
}

/// What came of a transcript.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    /// It waits for the merge window.
    Held,
    /// It joined the one held before it, which is no longer reported on its own: these turns are cancelled as merged.
    Joined(Vec<usize>),
    /// It is in another language than the one held before it, which is never joined to it: the held one is returned
    /// to be reported on its own, and this one is held in its place.
    Split(Pending),
}

impl Recognition {
    /// Queues a turn's audio and returns the job to start, if none is running. `Err` when it is a turn the queue does not
    /// hold yet and it is full: the job is dropped.
    pub(crate) fn push(&mut self, job: Job) -> Result<Option<Job>, Job> {
        if !self.holds(job.turn) && self.len() >= MAX_QUEUE {
            return Err(job);
        }
        self.queue.push_back(job);
        Ok(self.next())
    }

    /// Whether `turn` is the one in recognition.
    pub(crate) fn is_active(&self, turn: usize) -> bool {
        self.active.is_some_and(|(active, _)| active == turn)
    }

    /// Whether a job of `turn` waits for, or is in, recognition.
    pub(crate) fn holds(&self, turn: usize) -> bool {
        self.turns().contains(&turn)
    }

    /// Whether the job in recognition is the last piece of its turn.
    pub(crate) fn is_last(&self) -> bool {
        self.active.is_some_and(|(_, last)| last)
    }

    /// Ends the active job and returns the next one to start.
    pub(crate) fn done(&mut self) -> Option<Job> {
        self.active = None;
        self.next()
    }

    /// How many turns wait for, or are in, recognition.
    pub(crate) fn len(&self) -> usize {
        self.turns().len()
    }

    /// The turns waiting for, or in, recognition, in order, each once: a turn's pieces follow one another.
    fn turns(&self) -> Vec<usize> {
        let active = self.active.map(|(turn, _)| turn);
        let mut turns: Vec<usize> = active
            .into_iter()
            .chain(self.queue.iter().map(|job| job.turn))
            .collect();
        turns.dedup();
        turns
    }

    /// Holds a transcript for the merge window, joined to the one already held.
    pub(crate) fn hold(
        &mut self,
        turn: usize,
        text: String,
        language: Option<String>,
        deadline: u64,
    ) -> Outcome {
        let fresh = |text, language| Pending {
            turns: vec![turn],
            text,
            language,
            deadline,
        };
        match self.pending.take() {
            None => {
                self.pending = Some(fresh(text, language));
                Outcome::Held
            }
            Some(held) if held.language != language => {
                self.pending = Some(fresh(text, language));
                Outcome::Split(held)
            }
            Some(mut held) => {
                let joined = held.turns.clone();
                held.turns.push(turn);
                held.text = join(&held.text, &text);
                held.deadline = deadline;
                self.pending = Some(held);
                Outcome::Joined(joined)
            }
        }
    }

    /// When the held transcript is due: only while nothing else is on its way to join it, which `speaking` (a turn
    /// open) and the queue say.
    pub(crate) fn deadline(&self, speaking: bool) -> Option<u64> {
        let waiting = speaking || self.len() > 0;
        self.pending
            .as_ref()
            .filter(|_| !waiting)
            .map(|pending| pending.deadline)
    }

    /// The held transcript, if it is due at `now` (or at once, with `now` = `u64::MAX`).
    pub(crate) fn due(&mut self, now: u64, speaking: bool) -> Option<Pending> {
        let deadline = self.deadline(speaking)?;
        (now >= deadline).then(|| self.pending.take()).flatten()
    }

    /// Whether a transcript is held.
    pub(crate) fn holding(&self) -> bool {
        self.pending.is_some()
    }

    /// The held transcript, whatever its deadline.
    pub(crate) fn take_pending(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    /// Drops every job, the active one included, and returns their turns, each once.
    pub(crate) fn clear(&mut self) -> Vec<usize> {
        let turns = self.turns();
        self.active = None;
        self.queue.clear();
        turns
    }

    fn next(&mut self) -> Option<Job> {
        if self.active.is_some() {
            return None;
        }
        let job = self.queue.pop_front()?;
        self.active = Some((job.turn, job.last));
        Some(job)
    }
}

/// Everything but the transcript: its length stands for it.
impl fmt::Debug for Pending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pending")
            .field("turns", &self.turns)
            .field("text", &chars(&self.text))
            .field("language", &self.language)
            .field("deadline", &self.deadline)
            .finish()
    }
}
