//! The host a command runs on.
//!
//! Every operating system answers through the same maintained library (`sysinfo`), so the body is a default method
//! of [`Host`] and [`LocalHost`] adds nothing: no per-OS files.
use sysinfo::System;

/// What xtask can ask about the machine it runs on.
pub trait Host {
    /// One line naming the host name, the operating system, its version, the kernel and the CPU architecture.
    /// A part the host does not report reads `unknown`.
    fn description(&self) -> String {
        let part = |value: Option<String>| value.unwrap_or_else(|| "unknown".to_owned());
        format!(
            "{} {} {} (kernel {}, {})",
            part(System::host_name()),
            part(System::name()),
            part(System::os_version()),
            part(System::kernel_version()),
            System::cpu_arch()
        )
    }
}

/// The machine xtask runs on.
pub struct LocalHost;

impl Host for LocalHost {}

#[cfg(test)]
mod tests;
