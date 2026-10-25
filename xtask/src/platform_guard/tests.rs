use super::*;

#[test]
fn scans_multiline_cfgs_and_apis_but_ignores_literals_and_wasm() {
    let source = r#"
        #[cfg(
            any(unix, target_os = "windows")
        )]
        #[cfg_attr(target_family = "unix", allow(dead_code))]
        fn os() { cfg!(windows); std::os::fd; libc::x; rustix::x;
            windows_sys::x; windows::x; objc2::x; core_foundation::x; coreaudio::x; }
        #[cfg(target_family = "wasm")]
        #[cfg(target_arch = "wasm32")]
        fn host() { let _ = "libc::x cfg!(unix)"; }
        // std::os::fd cfg!(windows)
    "#;
    let mut aliases = BTreeSet::new();
    let build: TokenStream = "cfg_aliases! { apple: { target_os = \"macos\" }, desktop: { apple }, web: { target_arch = \"wasm32\" } }".parse().unwrap();
    collect_aliases(build.clone(), &mut aliases);
    collect_aliases(build, &mut aliases);
    assert_eq!(occurrences(source.parse().unwrap(), &aliases), 11);
    assert_eq!(
        occurrences(
            "#[cfg(desktop)] fn a() {} #[cfg(web)] fn b() {}"
                .parse()
                .unwrap(),
            &aliases
        ),
        1
    );
    assert!(exempt(Path::new("src/platform/memory/linux.rs")));
    assert!(exempt(Path::new("xtask/src/platform.rs")));
    assert!(!exempt(Path::new("tests/platform_test.rs")));
}

#[test]
fn grouped_and_renamed_imports_keep_the_original_os_names() {
    let source = "use std::{os::unix::fs::MetadataExt}; use std::os as host_os; use libc as c; use {rustix as r, windows_sys as w, windows as win, objc2 as o, core_foundation as cf, coreaudio as ca}; use std::{fs, path::Path};";
    assert_eq!(occurrences(source.parse().unwrap(), &BTreeSet::new()), 9);
}

#[test]
fn os_alias_definitions_fail_while_wasm_and_native_aliases_pass() {
    let source = r#"
        cfg_aliases! {
            review_os: { target_os = "windows" },
            posix: { unix },
            win: { windows },
            family: { target_family = "unix" },
            web: { target_arch = "wasm32" },
            native: { not(target_arch = "wasm32") },
            wasm_family: { target_family = "wasm" },
        }
    "#;
    assert_eq!(occurrences(source.parse().unwrap(), &BTreeSet::new()), 4);
    let host = r#"cfg_aliases! {
        web: { target_arch = "wasm32" },
        native: { not(target_arch = "wasm32") },
        wasm_family: { target_family = "wasm" },
    }"#;
    assert_eq!(occurrences(host.parse().unwrap(), &BTreeSet::new()), 0);
}
