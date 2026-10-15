//! Shared model resources are leased by loaded calls and in-flight requests, not by the provider.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{
    Models, SharedModelLoader, SharedModelResources, SharedModels, Speaker, Transcriber, Vad,
    VadFactory, VadFrame, VoiceModels,
};
use crate::async_trait;

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

struct Factory(Arc<AtomicUsize>);
struct Detector(usize);
struct Text {
    drops: Arc<AtomicUsize>,
    #[cfg(native)]
    gate: Option<Arc<Gate>>,
}
struct Speech;

impl Drop for Text {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(native)]
struct Gate {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

struct Loader {
    loads: AtomicUsize,
    streams: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    last: Mutex<Option<Weak<dyn Transcriber>>>,
    #[cfg(native)]
    gate: Option<Arc<Gate>>,
}

impl Loader {
    fn new() -> Self {
        Self {
            loads: AtomicUsize::new(0),
            streams: Arc::new(AtomicUsize::new(0)),
            drops: Arc::new(AtomicUsize::new(0)),
            last: Mutex::new(None),
            #[cfg(native)]
            gate: None,
        }
    }
    fn live(&self) -> bool {
        self.last
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|weak| weak.upgrade().is_some())
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl SharedModelLoader for Loader {
    async fn load(&self) -> Result<SharedModelResources, String> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let text: Arc<dyn Transcriber> = Arc::new(Text {
            drops: Arc::clone(&self.drops),
            #[cfg(native)]
            gate: self.gate.clone(),
        });
        *self.last.lock().unwrap() = Some(Arc::downgrade(&text));
        Ok(SharedModelResources {
            vad: Arc::new(Factory(Arc::clone(&self.streams))),
            transcriber: text,
            speaker: Arc::new(Speech),
            end_of_turn: None,
        })
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl VadFactory for Factory {
    async fn new_stream(&self) -> Result<Box<dyn Vad>, String> {
        Ok(Box::new(Detector(self.0.fetch_add(1, Ordering::SeqCst))))
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl Vad for Detector {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
        self.0 += pcm.len();
        Ok(vec![VadFrame {
            end: self.0 as u64,
            speech: false,
            probability: None,
        }])
    }
    async fn reset(&mut self) {
        self.0 = 0;
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl Transcriber for Text {
    async fn transcribe(&self, _: Vec<f32>, _: u32, _: Option<String>) -> Result<String, String> {
        #[cfg(native)]
        if let Some(gate) = &self.gate {
            gate.started.notify_one();
            gate.release.notified().await;
        }
        Ok(String::new())
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl Speaker for Speech {
    async fn speak(
        &self,
        _: String,
        _: Option<String>,
        _: Option<String>,
        _: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        Ok((Vec::new(), 16_000))
    }
}

async fn check_shared_leases() {
    let loader = Arc::new(Loader::new());
    let shared = SharedModels::new(Arc::clone(&loader) as Arc<dyn SharedModelLoader>);
    let Models {
        vad: mut a,
        transcriber: text_a,
        speaker: speech_a,
        ..
    } = shared.load().await.unwrap();
    let Models {
        vad: mut b,
        transcriber: text_b,
        speaker: speech_b,
        ..
    } = shared.load().await.unwrap();
    assert_eq!(
        loader.loads.load(Ordering::SeqCst),
        1,
        "two calls share one loaded choice"
    );
    assert_eq!(
        loader.streams.load(Ordering::SeqCst),
        2,
        "each call gets a fresh detector stream"
    );
    assert!(loader.live());
    assert_ne!(
        a.accept(&[0.0; 2]).await.unwrap(),
        b.accept(&[0.0; 2]).await.unwrap()
    );
    a.reset().await;
    assert_eq!(a.accept(&[0.0]).await.unwrap()[0].end, 1);
    assert_eq!(b.accept(&[0.0]).await.unwrap()[0].end, 4);
    // A call's existing idle timer releases its Models; one still-live lease prevents eviction.
    drop((a, text_a, speech_a));
    assert!(loader.live());
    drop((b, text_b, speech_b));
    assert!(
        !loader.live(),
        "the lightweight provider keeps no strong weight reference"
    );
    assert_eq!(loader.drops.load(Ordering::SeqCst), 1);
    let again = shared.load().await.unwrap();
    assert_eq!(
        loader.loads.load(Ordering::SeqCst),
        2,
        "restart after final idle unload reloads"
    );
    drop(again);
    assert_eq!(loader.drops.load(Ordering::SeqCst), 2);
}

#[cfg(native)]
#[test]
fn shared_leases_evict_and_reload_after_last_call_is_idle() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(check_shared_leases());
}

#[cfg(web)]
#[test]
async fn shared_leases_evict_and_reload_after_last_call_is_idle() {
    check_shared_leases().await;
}

#[cfg(native)]
#[test]
fn an_in_flight_transcription_keeps_weights_after_other_leases_end() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            let gate = Arc::new(Gate {
                started: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            });
            let mut source = Loader::new();
            source.gate = Some(Arc::clone(&gate));
            let loader = Arc::new(source);
            let shared = SharedModels::new(Arc::clone(&loader) as Arc<dyn SharedModelLoader>);
            let a = shared.load().await.unwrap();
            let b = shared.load().await.unwrap();
            let transcriber = Arc::clone(&a.transcriber);
            let task =
                tokio::spawn(
                    async move { transcriber.transcribe(vec![0.0; 16], 16_000, None).await },
                );
            gate.started.notified().await;
            drop(a);
            drop(b);
            assert!(loader.live(), "the active request owns the final lease");
            gate.release.notify_one();
            assert!(task.await.unwrap().is_ok());
            assert!(!loader.live(), "weights leave after the request finishes");
        });
}
