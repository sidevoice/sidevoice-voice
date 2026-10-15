//! A shared model lease creates independent detector streams without reloading the other slots.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::{Models, SharedModels, Speaker, Transcriber, Vad, VadFactory, VadFrame, VoiceModels};
use crate::async_trait;

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

struct Factory(Arc<AtomicUsize>);
struct Detector(usize);
struct Text;
struct Speech;

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

async fn check_shared_weights_have_distinct_detector_state() {
    let made = Arc::new(AtomicUsize::new(0));
    let shared = SharedModels {
        vad: Arc::new(Factory(Arc::clone(&made))),
        transcriber: Arc::new(Text),
        speaker: Arc::new(Speech),
        end_of_turn: None,
    };
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
    assert_eq!(made.load(Ordering::SeqCst), 2);
    assert!(Arc::ptr_eq(&text_a, &text_b) && Arc::ptr_eq(&speech_a, &speech_b));
    assert_ne!(
        a.accept(&[0.0; 2]).await.unwrap(),
        b.accept(&[0.0; 2]).await.unwrap()
    );
    a.reset().await;
    assert_eq!(a.accept(&[0.0]).await.unwrap()[0].end, 1);
    assert_eq!(b.accept(&[0.0]).await.unwrap()[0].end, 4);
}

#[cfg(native)]
#[test]
fn shared_weights_have_distinct_detector_state() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(check_shared_weights_have_distinct_detector_state());
}

#[cfg(web)]
#[test]
async fn shared_weights_have_distinct_detector_state() {
    check_shared_weights_have_distinct_detector_state().await;
}
