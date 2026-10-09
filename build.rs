//! The module's platform aliases, named once: `web` and `native`. They name platforms only; which of them a module
//! needs is that module's own `#[cfg]` to say.

use cfg_aliases::cfg_aliases;

fn main() {
    cfg_aliases! {
        web: { target_arch = "wasm32" },
        native: { not(target_arch = "wasm32") },
    }
}
