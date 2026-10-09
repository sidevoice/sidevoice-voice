//! The recognition queue: turns that ended wait here for their transcript, one at a time and in order, at most eight
//! of them. A transcript passes the acceptance filter (`filter`), then waits out the merge window: a turn that
//! follows within it is joined to it, so a sentence split by a pause is reported once.

mod filter;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;

pub(crate) use filter::accepted;

/// The most turns waiting for, or in, recognition.
pub(crate) const MAX_QUEUE: usize = 8;

/// A turn's audio, waiting for its transcript.
#[derive(Debug, PartialEq)]
pub(crate) struct Job {
    pub(crate) turn: usize,
    pub(crate) pcm: Vec<f32>,
}

/// A transcript held for the merge window.
#[derive(Debug, PartialEq)]
pub(crate) struct Pending {
    /// The turns it holds, oldest first; the last one carries the transcript.
    pub(crate) turns: Vec<usize>,
    pub(crate) text: String,
    deadline: u64,
}

/// The queue, the job in recognition, and the transcript held for the merge window.
#[derive(Debug, Default)]
pub(crate) struct Recognition {
    queue: VecDeque<Job>,
    active: Option<usize>,
    pending: Option<Pending>,
}

/// What came of a transcript.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    /// It waits for the merge window.
    Held,
    /// It joined the one held before it, which is no longer reported on its own: these turns are cancelled as merged.
    Joined(Vec<usize>),
}

impl Recognition {
    /// Queues a turn's audio and returns the job to start, if none is running. `None` when the queue is full: the turn
    /// is dropped.
    pub(crate) fn push(&mut self, job: Job) -> Result<Option<Job>, Job> {
        if self.queue.len() + usize::from(self.active.is_some()) >= MAX_QUEUE {
            return Err(job);
        }
        self.queue.push_back(job);
        Ok(self.next())
    }

    /// Whether `turn` is the one in recognition.
    pub(crate) fn is_active(&self, turn: usize) -> bool {
        self.active == Some(turn)
    }

    /// Ends the active job and returns the next one to start.
    pub(crate) fn done(&mut self) -> Option<Job> {
        self.active = None;
        self.next()
    }

    /// How many turns wait for, or are in, recognition.
    pub(crate) fn len(&self) -> usize {
        self.queue.len() + usize::from(self.active.is_some())
    }

    /// Holds a transcript for the merge window, joined to the one already held.
    pub(crate) fn hold(&mut self, turn: usize, text: String, deadline: u64) -> Outcome {
        match self.pending.take() {
            None => {
                self.pending = Some(Pending {
                    turns: vec![turn],
                    text,
                    deadline,
                });
                Outcome::Held
            }
            Some(mut held) => {
                let joined = held.turns.clone();
                held.turns.push(turn);
                held.text = format!("{} {text}", held.text);
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

    /// Drops every job, the active one included, and returns their turns.
    pub(crate) fn clear(&mut self) -> Vec<usize> {
        let mut turns: Vec<usize> = self.active.take().into_iter().collect();
        turns.extend(self.queue.drain(..).map(|job| job.turn));
        turns
    }

    fn next(&mut self) -> Option<Job> {
        if self.active.is_some() {
            return None;
        }
        let job = self.queue.pop_front()?;
        self.active = Some(job.turn);
        Some(job)
    }
}
