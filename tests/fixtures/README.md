Recorded speech the tests hear, 16 kHz mono 16-bit PCM WAV. The unit tests compile them in (`src/test_support.rs`),
so the wasm32 tests read them too. Real models are the apps' to test.

| File | What it is | Source | Licence | SHA-256 |
|---|---|---|---|---|
| `librispeech_mr_quilter.wav` | "Mister Quilter is the apostle of the middle classes and we are glad to welcome his gospel", English, 5.9 s | LibriSpeech dev-clean 1272-128104-0000, as `hf-internal-testing/dummy-audio-samples` (revision 798f2c7) serves it | CC-BY-4.0 | `799f78ed4beb4de7ceae3a809262d4ce242394342ccd1d58cef7d49dbc2def46` |
| `fleur_es_sample.wav` | "Esto parece tener sentido, ya que en la Tierra no se percibe su movimiento, ¿cierto?", Spanish, 7.7 s | FLEURS es_419 test, sentence 1770, as `hf-internal-testing/dummy-audio-samples` (revision 798f2c7) serves it | CC-BY-4.0 | `04bf4876d16ba5a867f093ba5b34e93f4b82fc8250e78cce90932950d5035132` |

The same clips are sidevoice-engine's recorded clips (its `tests/voice_loop.json`).
