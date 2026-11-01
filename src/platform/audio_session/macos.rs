use super::{AudioSession, SessionConfig};

/// macOS: no audio session; Core Audio takes whatever device is opened, and the speaker is chosen among the devices it lists.
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
