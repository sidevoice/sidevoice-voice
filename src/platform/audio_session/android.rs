use super::{AudioSession, SessionConfig};

/// Android: the session is not the process's to declare from native code. Its mode (`MODE_IN_COMMUNICATION`), its
/// routing and its audio focus belong to `AudioManager`, which a native library reaches only through the JVM, and
/// the capture preset to the stream cpal opens; the speaker is chosen among the devices cpal lists.
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
