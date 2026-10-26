use sysinfo::System;

use super::{Host, LocalHost};

#[test]
fn describes_the_host_on_one_line_with_the_architecture_sysinfo_reports() {
    let text = LocalHost.description();
    // The library spells the architecture its own way (`arm64`, not Rust's `aarch64`, on Apple and Windows ARM).
    assert!(text.contains(&System::cpu_arch()), "{text}");
    assert!(!text.contains('\n'), "{text}");
}

#[test]
fn names_the_host() {
    let name = System::host_name().unwrap_or_else(|| "unknown".to_owned());
    assert!(LocalHost.description().starts_with(&name));
}
