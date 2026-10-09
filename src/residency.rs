//! When the call's models leave memory: once they have been idle, loaded while the call is stopped, for the
//! configuration's `idle_unload_minutes` (0: as soon as it stops). Dropping them unloads them from the engine; the
//! next start loads them again.

#[cfg(test)]
mod tests;

/// The clock of the models' idleness, in the call's milliseconds.
#[derive(Debug)]
pub(crate) struct Residency {
    idle_ms: u64,
    /// Since when the models have been idle.
    idle_since: Option<u64>,
}

impl Residency {
    pub(crate) fn new(minutes: u32) -> Self {
        Self {
            idle_ms: minutes_ms(minutes),
            idle_since: None,
        }
    }

    pub(crate) fn set_minutes(&mut self, minutes: u32) {
        self.idle_ms = minutes_ms(minutes);
    }

    /// Takes whether the models are idle at `now`: loaded, with the call stopped.
    pub(crate) fn observe(&mut self, now: u64, idle: bool) {
        match (idle, self.idle_since) {
            (true, None) => self.idle_since = Some(now),
            (false, Some(_)) => self.idle_since = None,
            _ => {}
        }
    }

    /// When the models are due to leave memory, if they are idle.
    pub(crate) fn deadline(&self) -> Option<u64> {
        self.idle_since.map(|since| since + self.idle_ms)
    }

    /// Whether the models are due to leave memory at `now`; once it says so, the clock stops until they are idle again.
    pub(crate) fn due(&mut self, now: u64) -> bool {
        let due = self.deadline().is_some_and(|deadline| now >= deadline);
        if due {
            self.idle_since = None;
        }
        due
    }
}

fn minutes_ms(minutes: u32) -> u64 {
    u64::from(minutes) * 60_000
}
