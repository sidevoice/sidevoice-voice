//! Shared model resources are leased by loaded calls and in-flight requests, not by the provider.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{
    InputModels, OutputModels, SharedModelLoader, SharedModels, Speaker, Transcriber, Vad,
    VadFactory, VadFrame,
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
    /// How many times each slot was loaded: detector, transcriber, speaker.
    loads: [AtomicUsize; 3],
    streams: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    last: Mutex<Option<Weak<dyn Transcriber>>>,
    #[cfg(native)]
    gate: Option<Arc<Gate>>,
}

impl Loader {
    fn new() -> Self {
        Self {
            loads: Default::default(),
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
    fn loads(&self) -> [usize; 3] {
        self.loads
            .each_ref()
            .map(|loads| loads.load(Ordering::SeqCst))
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl SharedModelLoader for Loader {
    async fn vad(&self) -> Result<Arc<dyn VadFactory>, String> {
        self.loads[0].fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Factory(Arc::clone(&self.streams))))
    }

    async fn transcriber(&self) -> Result<Arc<dyn Transcriber>, String> {
        self.loads[1].fetch_add(1, Ordering::SeqCst);
        let text: Arc<dyn Transcriber> = Arc::new(Text {
            drops: Arc::clone(&self.drops),
            #[cfg(native)]
            gate: self.gate.clone(),
        });
        *self.last.lock().unwrap() = Some(Arc::downgrade(&text));
        Ok(text)
    }

    async fn speaker(&self) -> Result<Arc<dyn Speaker>, String> {
        self.loads[2].fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Speech))
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
    let (mut a, text_a, speech_a) = (
        shared.vad().await.unwrap(),
        shared.transcriber().await.unwrap(),
        shared.speaker().await.unwrap(),
    );
    let (mut b, text_b, speech_b) = (
        shared.vad().await.unwrap(),
        shared.transcriber().await.unwrap(),
        shared.speaker().await.unwrap(),
    );
    assert_eq!(
        loader.loads(),
        [1, 1, 1],
        "two calls share each loaded slot"
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
    // A call's existing idle timer releases its models; one still-live lease prevents eviction.
    drop((a, text_a, speech_a));
    assert!(loader.live());
    drop((b, text_b, speech_b));
    assert!(
        !loader.live(),
        "the lightweight provider keeps no strong weight reference"
    );
    assert_eq!(loader.drops.load(Ordering::SeqCst), 1);
    let again = shared.transcriber().await.unwrap();
    assert_eq!(
        loader.loads(),
        [1, 2, 1],
        "restart after final idle unload reloads"
    );
    drop(again);
    assert_eq!(loader.drops.load(Ordering::SeqCst), 2);
}

/// A transcription try (the input half) then a call (both halves) on one pool: the try loads no speaker, and the call
/// reuses the try's detector and transcriber while the try still holds them, loading only the speaker.
async fn check_halves_reuse_what_is_loaded() {
    let loader = Arc::new(Loader::new());
    let shared = SharedModels::new(Arc::clone(&loader) as Arc<dyn SharedModelLoader>);
    let input: &dyn InputModels = &shared;
    let output: &dyn OutputModels = &shared;
    let try_models = (
        input.vad().await.unwrap(),
        input.transcriber().await.unwrap(),
    );
    assert_eq!(loader.loads(), [1, 1, 0], "an input half loads no speaker");
    let call = (
        input.vad().await.unwrap(),
        input.transcriber().await.unwrap(),
        output.speaker().await.unwrap(),
    );
    assert_eq!(
        loader.loads(),
        [1, 1, 1],
        "no second load of what the try holds"
    );
    assert_eq!(
        input.end_of_turn().await.err().as_deref(),
        Some("end-of-turn-missing")
    );
    drop((try_models, call));
}

#[cfg(native)]
#[test]
fn halves_reuse_what_is_loaded() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(check_halves_reuse_what_is_loaded());
}

#[cfg(web)]
#[test]
async fn halves_reuse_what_is_loaded() {
    check_halves_reuse_what_is_loaded().await;
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
            let a = shared.transcriber().await.unwrap();
            let b = shared.transcriber().await.unwrap();
            let transcriber = Arc::clone(&a);
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
