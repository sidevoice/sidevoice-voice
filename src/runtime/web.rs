//! The browser's: tasks on its event loop, `setTimeout`, `performance.now()` and `Date.now()`, in a page or a
//! worker alike.
#![cfg(web)]

use std::future::Future;

use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = setTimeout)]
    fn set_timeout(callback: &Function, ms: f64) -> JsValue;
}

/// Runs `task` on the browser's event loop.
pub(crate) fn spawn(task: impl Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(task);
}

/// Waits `ms` milliseconds.
pub(crate) async fn sleep(ms: u64) {
    let promise = Promise::new(&mut |resolve, _| {
        set_timeout(&resolve, ms as f64);
    });
    let _ = JsFuture::from(promise).await;
}

/// `performance.now()`, in whole milliseconds.
pub(crate) fn monotonic_ms() -> u64 {
    let performance = Reflect::get(&js_sys::global(), &"performance".into()).unwrap_or_default();
    let now = Reflect::get(&performance, &"now".into())
        .ok()
        .and_then(|now| now.dyn_into::<Function>().ok())
        .and_then(|now| now.call0(&performance).ok())
        .and_then(|now| now.as_f64());
    now.unwrap_or_else(js_sys::Date::now) as u64
}

/// `Date.now()`.
pub(crate) fn unix_ms() -> u64 {
    js_sys::Date::now() as u64
}
