use super::{AudioSession, SessionConfig};

/// Linux: no audio session; ALSA and the sound server take whatever device is opened, and the speaker is chosen among the devices they list.
pub struct Os;

impl AudioSession for Os {
    const OUTPUT_SELECTABLE: bool = true;

    fn configure(_config: &SessionConfig) -> Result<(), String> {
        Ok(())
    }

    fn activate() -> Result<(), String> {
        Ok(())
    }

    fn deactivate() -> Result<(), String> {
        Ok(())
    }
}
