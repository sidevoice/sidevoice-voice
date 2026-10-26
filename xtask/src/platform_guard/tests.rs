use super::*;

fn n(source: &str) -> usize {
    scan(source, &Shared::default()).unwrap().count
}

#[test]
fn catches_os_code() {
    assert_eq!(
        n("#[cfg_attr(\n all(target_os = \"linux\", unix), ignore)]\nuse std::os::unix::fs::PermissionsExt;"),
        2
    );
    assert_eq!(
        n("fn f() { if cfg!(windows) {} let _ = std::env::consts::OS; }"),
        2
    );
    assert_eq!(n("fn f() { unsafe { libc::getpid() }; }"), 1);
}

#[test]
fn ignores_comments_strings_and_unrelated_names() {
    let source = "//! uses std::os::unix and #[cfg(unix)]\n/// libc::getpid()\nfn f(v: &[u8]) {\n // cfg!(windows)\n let _ = \"std::os::unix libc::x cfg(unix)\"; let _ = v.windows(2); let _ = r#\"Command::new(\"sh\")\"#;\n let _ = (cfg!(feature = \"windows\"), cfg!(test)); }";
    assert_eq!(n(source), 0);
}

#[test]
fn exempts_platform_files() {
    for path in ["src/platform/user/unix.rs", "xtask/src/platform.rs"] {
        assert!(exempt(Path::new(path)));
    }
}

#[test]
fn catches_grouped_and_nested_imports() {
    assert_eq!(n("use std::{os::unix::fs::PermissionsExt};"), 1);
    assert_eq!(n("use std::{fs, os::{unix::fs::PermissionsExt, raw}};"), 2);
}

#[test]
fn follows_import_aliases() {
    assert_eq!(n("use libc as sys; fn f() { sys::getpid(); }"), 2);
    assert_eq!(
        n("use std::os as o; fn f() -> o::unix::raw::pid_t { 0 }"),
        2
    );
    assert_eq!(
        n("use windows_sys::Win32::Foundation as f; fn g() { let _ = f::CloseHandle; }"),
        2
    );
    assert_eq!(n("extern crate libc as l; fn f() { l::getpid(); }"), 2);
}

#[test]
fn catches_os_in_macros_and_attribute_forms() {
    assert_eq!(n("fn f() { println!(\"{}\", libc::getpid()); }"), 1);
    assert_eq!(
        n("cfg_if::cfg_if! { if #[cfg(unix)] { fn a() {} } else { fn a() {} } }"),
        1
    );
    assert_eq!(n("#![cfg(target_os = \"macos\")]"), 1);
    assert_eq!(n("#![windows_subsystem = \"windows\"] #[cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")] fn main() {}"), 2);
}

#[test]
fn resolves_cfg_aliases_across_files() {
    let build = "fn main() { cfg_aliases::cfg_aliases! { apple: { target_vendor = \"apple\" }, mac: { all(apple, x) }, other: { feature = \"x\" } } }";
    let aliases = scan(build, &Shared::default()).unwrap();
    assert_eq!(aliases.count, 2);
    assert_eq!(
        aliases.shared.cfg_aliases,
        HashSet::from(["apple".to_string(), "mac".to_string()])
    );
    let user = "#[cfg(mac)] fn a() {} #[cfg(other)] fn b() {} fn c() { if cfg!(apple) {} }";
    assert_eq!(scan(user, &aliases.shared).unwrap().count, 2);
}

#[test]
fn catches_os_commands() {
    assert_eq!(
        n("fn f() { std::process::Command::new(\"uname\").arg(\"-a\"); }"),
        1
    );
    assert_eq!(
        n("fn f() { tokio::process::Command::new(\"/bin/sh\"); }"),
        1
    );
    assert_eq!(n("fn f() { Command::new(\"PowerShell.exe\"); }"), 1);
    assert_eq!(
        n("use std::process::Command as Cmd; fn f() { Cmd::new(\"launchctl\"); }"),
        1
    );
    assert_eq!(
        n("fn f() { Command::new(\"git\"); Command::new(\"cargo\"); }"),
        0
    );
}

#[test]
fn catches_cargo_os_variables() {
    assert_eq!(
        n("fn main() { let _ = std::env::var(\"CARGO_CFG_TARGET_OS\"); }"),
        1
    );
}

#[test]
fn rejects_unreadable_source() {
    assert!(scan("fn f() { \"unterminated }", &Shared::default()).is_err());
}

fn with_wrapper(source: &str) -> usize {
    let shared = Shared {
        command_types: HashSet::from(["Cmd".to_string()]),
        ..Shared::default()
    };
    scan(source, &shared).unwrap().count
}

#[test]
fn resolves_portable_parent_imports() {
    assert_eq!(n("use std::env; fn f() { let _ = env::consts::OS; }"), 1);
    assert_eq!(
        n("use std::env::consts; fn f() { let _ = consts::FAMILY; }"),
        1
    );
    assert_eq!(
        n("use std as s; fn f() { let _ = s::os::unix::fs::chown; }"),
        1
    );
    assert_eq!(
        n("use std::{env as e, os}; fn f() { let _ = e::consts::OS; os::unix::raw::pid_t::MAX; }"),
        3
    );
    // Portable uses of the same parents stay allowed.
    assert_eq!(n("use std::env; fn f() { let _ = env::var(\"PORTABLE_SETTING\"); let _ = env::consts::ARCH; }"), 0);
}

#[test]
fn sees_cfg_in_every_macro_delimiter() {
    assert_eq!(
        n("fn f() { let _ = cfg! { unix }; let _ = cfg![windows]; let _ = cfg!(target_os = \"linux\"); }"),
        3
    );
    let build = "fn main() { cfg_aliases::cfg_aliases! { mac: { target_os = \"macos\" } } }";
    let aliases = scan(build, &Shared::default()).unwrap();
    assert_eq!(
        scan(
            "fn f() { let _ = cfg![mac]; let _ = cfg! { mac }; }",
            &aliases.shared
        )
        .unwrap()
        .count,
        2
    );
}

#[test]
fn sees_commands_through_generics_raw_names_and_wrappers() {
    assert_eq!(
        n("fn f() { std::process::Command::new::<&str>(\"sh\"); }"),
        1
    );
    assert_eq!(
        n("use std::process::Command as r#Run; fn f() { r#Run::new(\"sh\"); }"),
        1
    );
    assert_eq!(
        n("use std::process::Command as Run; fn f() { Run::new(\"cmd.exe\"); }"),
        1
    );
    assert_eq!(
        with_wrapper("use crate::util::Cmd; fn f() { Cmd::new(\"powershell.exe\"); }"),
        1
    );
    assert_eq!(
        with_wrapper("use crate::util::Cmd; fn f() { Cmd::new(\"git\").arg(\"status\"); }"),
        0
    );
    let defined = "pub struct Cmd { command: Command, shown: Vec<String> } type Run = tokio::process::Command;";
    let learned = scan(defined, &Shared::default())
        .unwrap()
        .shared
        .command_types;
    assert_eq!(
        learned,
        HashSet::from(["Cmd".to_string(), "Run".to_string()])
    );
    assert_eq!(
        n("struct Plain { shown: Vec<String> } fn f() { Plain::new(\"sh\"); }"),
        0
    );
}

#[test]
fn catches_local_sockets() {
    assert_eq!(
        n("fn f() { let _ = tokio::net::UnixStream::connect(\"socket\"); }"),
        1
    );
    assert_eq!(n("use tokio::net::{TcpStream, UnixListener}; fn f() {}"), 1);
    assert_eq!(
        n("fn f() { let _ = tokio::net::windows::named_pipe::ClientOptions::new(); }"),
        1
    );
    assert_eq!(
        n("fn f() { let _ = tokio::net::TcpStream::connect(\"127.0.0.1:1\"); }"),
        0
    );
}

#[test]
fn catches_os_paths_in_operations_only() {
    assert_eq!(
        n("fn f() { let _ = std::fs::read_to_string(\"/proc/self/status\"); }"),
        1
    );
    assert_eq!(
        n("fn f() { let _ = std::path::PathBuf::from(\"C:\\\\Windows\\\\x\"); }"),
        1
    );
    assert_eq!(
        n("fn f(c: &mut C) { c.args([\"-o\", \"/dev/null\"]).arg(r\"\\\\.\\pipe\\x\"); }"),
        2
    );
    assert_eq!(n("fn f() { std::fs::read(Path::new(\"/etc/hosts\")); }"), 1);
    // Documentation and data are not operations.
    assert_eq!(n("/// Reads /proc/self/status.\nfn f() { let _ = \"/proc/self/status\"; let _ = String::from(\"/dev/null\"); }"), 0);
    assert_eq!(
        n("fn f() { std::fs::read_to_string(\"notes/readme.txt\"); }"),
        0
    );
}

#[test]
fn catches_native_framework_links() {
    assert_eq!(
        n("#[link(name = \"AppKit\", kind = \"framework\")] extern \"C\" {}"),
        1
    );
    assert_eq!(n("#[cfg_attr(feature = \"x\", link(name = \"AppKit\", kind = \"framework\"))] extern \"C\" {}"), 1);
    assert_eq!(n("#[link(name = \"m\")] extern \"C\" {}"), 0);
}

#[test]
fn counts_cargo_variables_only_when_read() {
    assert_eq!(
        n("fn main() { let _ = std::env::var_os(\"CARGO_CFG_TARGET_FAMILY\"); }"),
        1
    );
    assert_eq!(
        n("use std::env::var as get; fn main() { let _ = get(\"CARGO_CFG_UNIX\"); }"),
        1
    );
    assert_eq!(
        n("use std::env; fn main() { let _ = env::var(\"CARGO_CFG_UNIX\"); }"),
        1
    );
    assert_eq!(
        n("const X: &str = env!(\"CARGO_CFG_TARGET_OS\"); fn f() { option_env![\"CARGO_CFG_WINDOWS\"]; }"),
        2
    );
    assert_eq!(n("const EXAMPLE: &str = \"CARGO_CFG_TARGET_OS\"; #[test] fn f() { assert_eq!(name(), \"CARGO_CFG_TARGET_OS\"); }"), 0);
}

fn root(source: &str) -> usize {
    scan_platform(source, true, &Shared::default())
        .unwrap()
        .count
}

fn other(source: &str) -> usize {
    scan_platform(source, false, &Shared::default())
        .unwrap()
        .count
}

#[test]
fn platform_root_rejects_os_cfg_and_unused_allows() {
    assert_eq!(
        root("#[cfg(target_os = \"macos\")] mod tray; #[cfg(windows)] mod tray;"),
        2
    );
    assert_eq!(root("#[cfg_attr(unix, derive(Debug))] pub struct A;"), 1);
    assert_eq!(
        root("#![allow(dead_code)] #[allow(unused_imports)] use a::b; #[expect(unused)] fn f() {}"),
        3
    );
    assert_eq!(root("#[cfg_attr(test, allow(dead_code))] fn f() {}"), 1);
    assert_eq!(
        root("#[cfg_attr(not(debug_assertions), cfg_attr(unix, allow(dead_code)))] fn f() {}"),
        1
    );
}

#[test]
fn platform_root_accepts_portable_declarations() {
    assert_eq!(
        root("mod tray; pub use tray::Tray; #[cfg(test)] mod tests {} #[allow(clippy::needless_return)] fn f() {}"),
        0
    );
    assert_eq!(
        root("#[cfg(feature = \"windows\")] mod a; #[derive(Debug)] struct B; #[deny(dead_code)] fn f() {}"),
        0
    );
    assert_eq!(
        root("/// Uses #[cfg(unix)] and allow(dead_code).\nfn f() { let _ = \"#[cfg(unix)]\"; }"),
        0
    );
}

#[test]
fn root_rules_apply_to_mod_rs_only() {
    assert_eq!(
        other("#[cfg(target_os = \"macos\")] mod mac; #[allow(dead_code)] fn f() {}"),
        0
    );
    assert!(platform_root(Path::new("src-tauri/src/platform/mod.rs")));
    assert!(!platform_root(Path::new("src-tauri/src/platform/tray.rs")));
    assert!(!platform_root(Path::new(
        "src-tauri/src/platform/tray/mod.rs"
    )));
    assert!(!platform_root(Path::new("src-tauri/src/other/mod.rs")));
}

#[test]
fn platform_roots_are_platform_rs_and_platform_mod_rs() {
    assert!(platform_root(Path::new("src/platform.rs")));
    assert!(platform_root(Path::new("xtask/src/platform.rs")));
    assert!(platform_root(Path::new("src/platform/mod.rs")));
    assert!(!platform_root(Path::new("src/platform/memory.rs")));
    assert!(!platform_root(Path::new("src/other/mod.rs")));
}

#[test]
fn framework_crate_families_are_os_apis() {
    assert_eq!(n("use objc2_foundation::NSString;"), 1);
    assert_eq!(n("use cocoa::appkit::NSApp;"), 1);
    assert_eq!(
        n("use objc2_app_kit::NSApplication as App; fn f() { App::sharedApplication(); }"),
        2
    );
    assert_eq!(
        n("use {core_foundation_sys as cf, coreaudio_sys as ca, core_graphics::display};"),
        3
    );
    assert_eq!(
        n("fn f() { windows_sys::Win32::Foundation::CloseHandle(0); }"),
        1
    );
}

#[test]
fn wasm_and_native_conditions_are_not_os() {
    let host = r#"
        #[cfg(target_family = "wasm")]
        #[cfg(not(target_family = "wasm"))]
        #[cfg(target_arch = "wasm32")]
        #[cfg(not(target_arch = "wasm32"))]
        fn f() { let _ = (cfg!(target_family = "wasm"), v.windows(2)); }
    "#;
    assert_eq!(n(host), 0);
    assert_eq!(
        n("#[cfg(any(target_family = \"wasm\", unix))] fn f() {}"),
        1
    );
    let build = "cfg_aliases! { web: { target_arch = \"wasm32\" }, native: { not(target_arch = \"wasm32\") }, wasm_family: { target_family = \"wasm\" } }";
    assert_eq!(n(build), 0);
}

#[test]
fn command_line_functions_are_followed_across_files() {
    let helpers = "fn run_in(dir: &Path, words: &str) -> String { Command::new(words).output() }\nfn sh(words: &str) -> String { run_in(&repo(), words) }";
    let shared = scan(helpers, &Shared::default()).unwrap().shared;
    let mut shared = scan(helpers, &shared).unwrap().shared;
    assert_eq!(
        shared.command_fns,
        HashSet::from(["run_in".to_string(), "sh".to_string()])
    );
    let count = |source: &str| scan(source, &shared).unwrap().count;
    assert_eq!(count("fn f() { sh(\"uname -a\"); }"), 1);
    assert_eq!(count("fn f() { run_in(&root, \"/bin/sh -c x\", &[]); }"), 1);
    assert_eq!(
        count("fn f() { sh(\"git rev-parse HEAD\"); sh(\"rustc --version\"); }"),
        0
    );
    shared.command_fns.clear();
    assert_eq!(
        scan("fn f() { sh(\"uname -a\"); }", &shared).unwrap().count,
        0
    );
}
