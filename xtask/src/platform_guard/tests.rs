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
fn debt_allows_existing_or_lower_counts_but_rejects_growth_and_new_files() {
    let debt = BTreeMap::from([("old.rs".into(), 2), ("removed.rs".into(), 1)]);
    assert!(compare(&BTreeMap::from([("old.rs".into(), 2)]), &debt).is_empty());
    assert!(compare(&BTreeMap::from([("old.rs".into(), 1)]), &debt).is_empty());
    let errors = compare(
        &BTreeMap::from([("old.rs".into(), 3), ("new.rs".into(), 1)]),
        &debt,
    );
    assert_eq!(errors.len(), 2);
}

#[test]
fn debt_cannot_add_files_or_raise_base_allowances() {
    let base = BTreeMap::from([("old.rs".into(), 2)]);
    assert!(shrink_errors(&BTreeMap::from([("old.rs".into(), 1)]), &base).is_empty());
    assert!(shrink_errors(&base, &base).is_empty());
    assert_eq!(
        shrink_errors(
            &BTreeMap::from([("old.rs".into(), 3), ("new.rs".into(), 0)]),
            &base
        )
        .len(),
        2
    );
}

#[test]
fn grouped_and_renamed_imports_keep_the_original_os_names() {
    let source = "use std::{os::unix::fs::MetadataExt}; use std::os as host_os; use libc as c; use {rustix as r, windows_sys as w, windows as win, objc2 as o, core_foundation as cf, coreaudio as ca}; use std::{fs, path::Path};";
    assert_eq!(occurrences(source.parse().unwrap(), &BTreeSet::new()), 9);
}
