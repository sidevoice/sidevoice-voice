//! The host a command runs on, described by a maintained portable library.
//! Nothing differs per operating system, so this is a plain module rather than a trait.
use sysinfo::System;

/// One line naming the operating system, its version, the kernel and the CPU architecture.
/// A part the host does not report reads `unknown`.
pub fn description() -> String {
    let part = |value: Option<String>| value.unwrap_or_else(|| "unknown".to_owned());
    format!(
        "{} {} (kernel {}, {})",
        part(System::name()),
        part(System::os_version()),
        part(System::kernel_version()),
        System::cpu_arch()
    )
}

#[cfg(test)]
mod tests;
