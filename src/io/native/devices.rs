//! Which microphone and speaker the native call uses ([`NativeDevices`]): the system's, listed by cpal, each known by
//! its cpal device id as text, and chosen at any time through the handle. The handle is shared with the audio thread,
//! which opens what is chosen and switches when it changes; a chosen device that goes away is the default one from
//! then, said once with the next list.

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;

use crate::platform::audio_session::{AudioSession, Session};

/// A microphone or speaker as the system lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AudioDevice {
    /// The cpal device id, as text: what [`NativeDevices::choose`] takes.
    pub id: String,
    pub label: String,
}

/// A chosen device that went away, by kind: the default one has been used since.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LostDevices {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

impl LostDevices {
    fn is_empty(&self) -> bool {
        self.input.is_none() && self.output.is_none()
    }
}

/// The system's microphones and speakers, and which are chosen (`None`: the system's default, not an entry).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
    pub input: Option<String>,
    pub output: Option<String>,
    /// Whether the speaker can be chosen at all: false where the system picks the output route (iOS), whatever
    /// `outputs` lists.
    #[serde(rename = "outputSelectable")]
    pub output_selectable: bool,
    /// Once, on the list after a chosen device went away.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lost: Option<LostDevices>,
}

/// A microphone or a speaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Input,
    Output,
}

/// What is chosen: `None` is the system's default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Choice {
    pub(crate) input: Option<String>,
    pub(crate) output: Option<String>,
}

impl Choice {
    fn of(&mut self, kind: Kind) -> &mut Option<String> {
        match kind {
            Kind::Input => &mut self.input,
            Kind::Output => &mut self.output,
        }
    }
}

#[derive(Default)]
struct Shared {
    choice: Mutex<Choice>,
    lost: Mutex<LostDevices>,
    input_switch: AtomicBool,
    output_switch: AtomicBool,
}

/// A handle on which devices the native call uses: a clone goes to
/// [`NativeIo::with_devices`](super::NativeIo::with_devices), and the other clones list and choose.
#[derive(Clone, Default)]
pub struct NativeDevices {
    shared: Arc<Shared>,
}

impl NativeDevices {
    /// The system's default microphone and speaker, until others are chosen.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The system's devices now, and which are chosen. A chosen one no longer listed is the default from now.
    ///
    /// # Errors
    ///
    /// A stable code when the system cannot list its devices.
    pub fn list(&self) -> Result<AudioDevices, String> {
        let (inputs, outputs) = listed()?;
        Ok(self.report(inputs, outputs))
    }

    /// Chooses the microphone, the speaker or both (`Some(None)`: the system's default; `None`: as it is). A call that
    /// runs switches: its microphone at once, its speaker once what is playing has ended.
    ///
    /// # Errors
    ///
    /// `audio-device-unknown` for an id the system does not list, the choice unchanged; or a listing failure.
    pub fn choose(
        &self,
        input: Option<Option<String>>,
        output: Option<Option<String>>,
    ) -> Result<AudioDevices, String> {
        let (inputs, outputs) = listed()?;
        for (wanted, list) in [(&input, &inputs), (&output, &outputs)] {
            if let Some(Some(id)) = wanted {
                if !list.iter().any(|device| &device.id == id) {
                    return Err("audio-device-unknown".into());
                }
            }
        }
        {
            let mut choice = lock(&self.shared.choice);
            for (kind, wanted) in [(Kind::Input, input), (Kind::Output, output)] {
                if let Some(id) = wanted {
                    if *choice.of(kind) != id {
                        *choice.of(kind) = id;
                        self.switch(kind);
                    }
                }
            }
        }
        Ok(self.report(inputs, outputs))
    }

    /// What is chosen now.
    pub(crate) fn chosen(&self) -> Choice {
        lock(&self.shared.choice).clone()
    }

    /// Whether `kind` was chosen anew since the last time this was asked.
    pub(crate) fn take_switch(&self, kind: Kind) -> bool {
        self.flag(kind).swap(false, Ordering::AcqRel)
    }

    /// The chosen `kind` went away: the default one from now, said once with the next list. True when it was chosen.
    pub(crate) fn lose(&self, kind: Kind) -> bool {
        let mut choice = lock(&self.shared.choice);
        let Some(id) = choice.of(kind).take() else {
            return false;
        };
        let mut lost = lock(&self.shared.lost);
        match kind {
            Kind::Input => lost.input = Some(id),
            Kind::Output => lost.output = Some(id),
        }
        self.switch(kind);
        true
    }

    fn flag(&self, kind: Kind) -> &AtomicBool {
        match kind {
            Kind::Input => &self.shared.input_switch,
            Kind::Output => &self.shared.output_switch,
        }
    }

    fn switch(&self, kind: Kind) {
        self.flag(kind).store(true, Ordering::Release);
    }

    /// The list as it is: a chosen device not listed is lost, and what was lost is said once.
    fn report(&self, inputs: Vec<AudioDevice>, outputs: Vec<AudioDevice>) -> AudioDevices {
        let gone = settle(&lock(&self.shared.choice), &inputs, &outputs);
        for kind in gone {
            self.lose(kind);
        }
        let choice = self.chosen();
        let lost = std::mem::take(&mut *lock(&self.shared.lost));
        AudioDevices {
            inputs,
            outputs,
            input: choice.input,
            output: choice.output,
            output_selectable: Session::OUTPUT_SELECTABLE,
            lost: (!lost.is_empty()).then_some(lost),
        }
    }
}

/// The kinds whose chosen device is not among those listed.
pub(crate) fn settle(
    choice: &Choice,
    inputs: &[AudioDevice],
    outputs: &[AudioDevice],
) -> Vec<Kind> {
    let missing = |id: &Option<String>, list: &[AudioDevice]| {
        id.as_ref()
            .is_some_and(|id| !list.iter().any(|device| &device.id == id))
    };
    let mut gone = Vec::new();
    if missing(&choice.input, inputs) {
        gone.push(Kind::Input);
    }
    if missing(&choice.output, outputs) {
        gone.push(Kind::Output);
    }
    gone
}

/// The system's microphones and speakers.
fn listed() -> Result<(Vec<AudioDevice>, Vec<AudioDevice>), String> {
    let host = cpal::default_host();
    let describe = |device: cpal::Device| {
        let id = device.id().ok()?.to_string();
        let label = device
            .description()
            .map(|description| description.name().to_owned())
            .unwrap_or_else(|_| id.clone());
        Some(AudioDevice { id, label })
    };
    let inputs = host
        .input_devices()
        .map_err(|_| "audio-devices-unavailable".to_owned())?
        .filter_map(describe)
        .collect();
    let outputs = host
        .output_devices()
        .map_err(|_| "audio-devices-unavailable".to_owned())?
        .filter_map(describe)
        .collect();
    Ok((inputs, outputs))
}

/// The device `id` (as [`AudioDevice::id`] gives it) of `host`, or `None` when it is not there.
pub(crate) fn by_id(host: &cpal::Host, id: &str) -> Option<cpal::Device> {
    host.device_by_id(&id.parse().ok()?)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
