# Changelog

## [0.5.2](https://github.com/sidevoice/sidevoice-voice/compare/v0.5.1...v0.5.2) (2026-10-16)


### Bug Fixes

* **turns:** barge-in at normal speaking volume on desktop ([#44](https://github.com/sidevoice/sidevoice-voice/issues/44)) ([4f102fb](https://github.com/sidevoice/sidevoice-voice/commit/4f102fb7b5315c574f42158b4248771828c237ec)), closes [#41](https://github.com/sidevoice/sidevoice-voice/issues/41)

## [0.5.1](https://github.com/sidevoice/sidevoice-voice/compare/v0.5.0...v0.5.1) (2026-10-16)


### Bug Fixes

* **playback:** play the shared native output without seams, underrun endings or a late cut ([#42](https://github.com/sidevoice/sidevoice-voice/issues/42)) ([f49bf09](https://github.com/sidevoice/sidevoice-voice/commit/f49bf09e26eba63717e65a58810cd9cf450692d3))

## [0.5.0](https://github.com/sidevoice/sidevoice-voice/compare/v0.4.0...v0.5.0) (2026-10-15)


### Features

* **call:** end the open turn now with endInput ([#35](https://github.com/sidevoice/sidevoice-voice/issues/35)) ([0a44c08](https://github.com/sidevoice/sidevoice-voice/commit/0a44c0820311ec562df26f100ff294d2a77d27ff))
* **turns:** gate interruption on continuous speech ([#31](https://github.com/sidevoice/sidevoice-voice/issues/31)) ([f0b113d](https://github.com/sidevoice/sidevoice-voice/commit/f0b113d196816a2d6cbb5c8fe5c4384680f1c49c))

## [0.4.0](https://github.com/sidevoice/sidevoice-voice/compare/v0.3.1...v0.4.0) (2026-10-14)


### ⚠ BREAKING CHANGES

* a start can keep its take: the audio heard, and where each turn's speech started, paused and ended ([#28](https://github.com/sidevoice/sidevoice-voice/issues/28))

### Features

* a start can keep its take: the audio heard, and where each turn's speech started, paused and ended ([#28](https://github.com/sidevoice/sidevoice-voice/issues/28)) ([5434aad](https://github.com/sidevoice/sidevoice-voice/commit/5434aad1330e83443329ce428cde97dcc9c49af2))
* **host:** publish rendered playback level ([#30](https://github.com/sidevoice/sidevoice-voice/issues/30)) ([618c900](https://github.com/sidevoice/sidevoice-voice/commit/618c9008a9062782ffea389625ca68b1c27ec387)), closes [#29](https://github.com/sidevoice/sidevoice-voice/issues/29)
* **turns:** a pause in the person's speech is told: the state says listening until they speak again ([#27](https://github.com/sidevoice/sidevoice-voice/issues/27)) ([a0f29c5](https://github.com/sidevoice/sidevoice-voice/commit/a0f29c5b37b17c42d47a714ddaf3918a9bd27ce4))


### Bug Fixes

* **turns:** speech and silence come from Silero, and smart turn is asked at every pause, as Pipecat did ([#25](https://github.com/sidevoice/sidevoice-voice/issues/25)) ([7dd02b9](https://github.com/sidevoice/sidevoice-voice/commit/7dd02b9876560d85a121027187b0ae7b86fa3b4a))

## [0.3.1](https://github.com/sidevoice/sidevoice-voice/compare/v0.3.0...v0.3.1) (2026-10-11)


### Bug Fixes

* **web:** a capture worklet whose processor throws fails the capture as audio-capture-failed ([#16](https://github.com/sidevoice/sidevoice-voice/issues/16)) ([852ce34](https://github.com/sidevoice/sidevoice-voice/commit/852ce345ebf6298b4de9cb982ae8ddb7922aea25))
* **web:** a microphone whose track the browser mutes is told in the call's state ([#17](https://github.com/sidevoice/sidevoice-voice/issues/17)) ([ddf7ba0](https://github.com/sidevoice/sidevoice-voice/commit/ddf7ba056f5038aff544022d767b31dd1a5b0053))

## [0.3.0](https://github.com/sidevoice/sidevoice-voice/compare/v0.2.0...v0.3.0) (2026-10-11)


### Features

* **native:** choose the call's microphone and speaker, mid-call too ([#19](https://github.com/sidevoice/sidevoice-voice/issues/19)) ([a03cdf1](https://github.com/sidevoice/sidevoice-voice/commit/a03cdf196b63e429eec82eabb76945589ffdd995))
* **web:** choose the call's microphone and output, mid-call too ([#18](https://github.com/sidevoice/sidevoice-voice/issues/18)) ([a2d2677](https://github.com/sidevoice/sidevoice-voice/commit/a2d26773b88ba467bf7a304bb4f6fdcd7a1fa234))

## [0.2.0](https://github.com/sidevoice/sidevoice-voice/compare/v0.1.0...v0.2.0) (2026-10-10)


### ⚠ BREAKING CHANGES

* **web:** the voice seam lists no models and keeps no keys; a slot's model choice is the page's ([#14](https://github.com/sidevoice/sidevoice-voice/issues/14))

### Features

* **web:** the voice seam lists no models and keeps no keys; a slot's model choice is the page's ([#14](https://github.com/sidevoice/sidevoice-voice/issues/14)) ([a6f2c0a](https://github.com/sidevoice/sidevoice-voice/commit/a6f2c0a22a878f5838c688e503c5b4db2318f685))

## 0.1.0 (2026-10-10)


### Features

* **native:** the device's microphone and speaker with cpal, and WebRTC AEC3 between them ([#3](https://github.com/sidevoice/sidevoice-voice/issues/3)) ([68145f8](https://github.com/sidevoice/sidevoice-voice/commit/68145f879b40c316667bb93a39050216fbd4b6cc))
* the call: a pure state machine, its task, and the app's models through its own interfaces ([#2](https://github.com/sidevoice/sidevoice-voice/issues/2)) ([55f95c2](https://github.com/sidevoice/sidevoice-voice/commit/55f95c248e67c95b3cdabe249c6754e4c58457fd))
* **web:** echoCancellation option for the browser's microphone ([#7](https://github.com/sidevoice/sidevoice-voice/issues/7)) ([f36ecb5](https://github.com/sidevoice/sidevoice-voice/commit/f36ecb56ea87943e4dd98acb03c57b0802223d90))
* **web:** the browser's microphone and speaker, and the npm package @sidevoice/voice ([#4](https://github.com/sidevoice/sidevoice-voice/issues/4)) ([4cbd1f6](https://github.com/sidevoice/sidevoice-voice/commit/4cbd1f6423f9ec98637f8ba32e1987b5016c51ae))
* **web:** the voice seam, VoiceHost, and createVoiceHost over the call ([#6](https://github.com/sidevoice/sidevoice-voice/issues/6)) ([9f1b7f6](https://github.com/sidevoice/sidevoice-voice/commit/9f1b7f6d3b312c9f29c399cafe1e23fffe414bd5))


### Bug Fixes

* **recognition:** take a language's script from CLDR, drop the logprob floors ([#9](https://github.com/sidevoice/sidevoice-voice/issues/9)) ([ec38aab](https://github.com/sidevoice/sidevoice-voice/commit/ec38aabc7e81ebbb836da27b36d652857996e84e)), closes [#8](https://github.com/sidevoice/sidevoice-voice/issues/8)
