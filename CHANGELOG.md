# Changelog

## 0.1.0 (2026-10-10)


### Features

* **native:** the device's microphone and speaker with cpal, and WebRTC AEC3 between them ([#3](https://github.com/sidevoice/sidevoice-voice/issues/3)) ([68145f8](https://github.com/sidevoice/sidevoice-voice/commit/68145f879b40c316667bb93a39050216fbd4b6cc))
* the call: a pure state machine, its task, and the app's models through its own interfaces ([#2](https://github.com/sidevoice/sidevoice-voice/issues/2)) ([55f95c2](https://github.com/sidevoice/sidevoice-voice/commit/55f95c248e67c95b3cdabe249c6754e4c58457fd))
* **web:** echoCancellation option for the browser's microphone ([#7](https://github.com/sidevoice/sidevoice-voice/issues/7)) ([f36ecb5](https://github.com/sidevoice/sidevoice-voice/commit/f36ecb56ea87943e4dd98acb03c57b0802223d90))
* **web:** the browser's microphone and speaker, and the npm package @sidevoice/voice ([#4](https://github.com/sidevoice/sidevoice-voice/issues/4)) ([4cbd1f6](https://github.com/sidevoice/sidevoice-voice/commit/4cbd1f6423f9ec98637f8ba32e1987b5016c51ae))
* **web:** the voice seam, VoiceHost, and createVoiceHost over the call ([#6](https://github.com/sidevoice/sidevoice-voice/issues/6)) ([9f1b7f6](https://github.com/sidevoice/sidevoice-voice/commit/9f1b7f6d3b312c9f29c399cafe1e23fffe414bd5))


### Bug Fixes

* **recognition:** take a language's script from CLDR, drop the logprob floors ([#9](https://github.com/sidevoice/sidevoice-voice/issues/9)) ([ec38aab](https://github.com/sidevoice/sidevoice-voice/commit/ec38aabc7e81ebbb836da27b36d652857996e84e)), closes [#8](https://github.com/sidevoice/sidevoice-voice/issues/8)
