# PCM audio

Version 1 declarations and Rust conversion/validation belong to
`neocogi-platform-contracts::render::platform` and its `audio` module.
`neocogi-platform::audio` implements native SDL and browser Web Audio. Enable
`audio` for audio-only Rust applications without Naga, graphics or windows.
`platform` includes audio through the existing platform root/library; no additional
crate or plugin is deployed.
Linked Rust applications can import the canonical values and clients directly
from `neocogi_platform::audio`; dynamic clients use the contracts crate.

## Rust types and system queries

Numerical parameters belong to the Rust enum variant:

```rust
PcmType::UnsignedInteger(8)
PcmType::SignedInteger(16)
PcmType::FixedPoint(32, 16)
PcmType::FloatingPoint(64)
```

Integer/fixed storage supports packed 8/16/24/32 bits. Fixed fractional bits range
from zero through width-1. Floats support IEEE binary32/binary64. `PcmFormat`
combines the variant with `PcmByteOrder::{LittleEndian, BigEndian}`, without
independent width/scale fields. `AudioChannels::{Mono, Stereo}` determines layout;
stereo samples are left, right, without implicit native byte order or padding.

`audio::capabilities()` or `Platform::audio_capabilities()` returns an immutable
`IAudioCapabilities`. Rust readers return ordinary vectors of concrete supported
PCM, byte-order and layout variants. The header combines `AudioBackend`, typed
`AudioSupport` for each direction and inclusive `AudioLimits`. Available support
carries an access policy and `AudioDeviceCount::{Unknown, Known(count), Failed(status)}`.
Unsupported APIs report Unavailable; there are no masks or sentinel counts.

Queries never open a microphone/window or running context. SDL initializes only
its audio subsystem for enumeration. Browser counts remain Unknown, with explicit
microphone permission/secure-context and output user-activation policies.
Capabilities describe application-side conversion, not hardware precision or a
guarantee that opening succeeds. Default system devices are opened. Codecs,
mixing, device selection, duplex-clock synchronization and hotplug renegotiation
remain outside this portable service.

## Ownership and lifecycle

The application owns one COM host as for other services. Opening returns an
exclusive `IAudioOpenRequest`; native construction completes immediately, while
browser permission/worklet initialization may remain Pending. Poll from the
application event/async loop. Failed carries its status; message returns bounded,
independently owned text. A matching typed take transfers one Ready stream once;
a wrong-direction NOINTERFACE preserves it. Playback/capture QI cannot recover the
opposite capability. Ready streams start Paused.

Streams retain SDL/browser lifetime independently of the platform root/windows.
Cancellation/drop closes untaken devices and invalidates pending completion.
Tracks from late permission are stopped without publication. Close is idempotent;
final release also stops tracks, disconnects nodes and unregisters callbacks before
releasing their Rust closures. Partial construction owns cleanup across every await.

Start/pause retain PCM and are idempotent in completed phases. Browser promises
expose Starting/Pausing. Clear requires Paused and preserves lifetime counters;
browsers expose Clearing until a generation acknowledgement. Pending controls
reject conflicts with Pending. Closed/Failed are terminal except for close.
`AudioInfo` contains either Playback queue/starvation data or Capture queue/overrun
data, never irrelevant counters for both directions.

## PCM and bounds

Rates span 8000–192000 Hz, queues 128–65536 complete frames, and each transfer at
most 4096 frames. Complete admission precedes initialization and permission.
Samples must decode to finite normalized [-1,1]. Unsigned silence is the midpoint;
signed integers divide by 2^(bits-1), fixed point by 2^fractional_bits. Capture
encoding rounds/saturates integer/fixed endpoints. Backends process float32;
integer32/floating64 storage does not promise bit-perfect 32/64-bit device precision.

Rust `write_f32`/`read_f32` use ordinary sample slices/vectors with the chosen
encoding. Raw transfers use explicitly encoded PCM bytes, zero offsets and whole
frames. Invalid requests write/consume nothing. Playback commits all or returns
Limit without waiting for space. Capture returns available complete frames,
including zero, initializing only that prefix. Excess newest capture frames drop.

SDL callbacks reserve queues before opening, allocate no Rust storage and invoke
no application/COM code. Application copies use SDL's required short device lock.
The bundled private AudioWorklet uses bounded output storage and capture credits
including messages in flight. Consumption returns output credits; idle statistics
allow one outstanding request. Epochs reject old reset packets. Browser backlog
can conservatively overestimate consumed output. No npm/Node or separately
installed worklet asset is required.

## Qualification

```sh
cargo test --locked -p neocogi-platform-contracts --features build-support,test-support
cargo test --locked -p neocogi-platform --no-default-features --features audio --test audio
cargo test --locked -p neocogi-platform --test gpu_backend_features
cargo tools test-browser --browser /path/to/chrome
```

Native acceptance uses isolated SDL dummy devices and invalid video, never a
physical microphone. Browser acceptance uses fake devices/permission in a fresh
profile and tests actual worklet PCM, denial, late cancellation and track teardown.
A real-time load-event barrier prevents DOM capture from outrunning asynchronous
audio. Dynamic acceptance opens both directions from one relocated platform
library without a window. The external archive consumer independently generates
platform bindings and exercises PCM enum payloads through COM on nightly/stable.

Underlying rules follow the [SDL device-lock contract](https://wiki.libsdl.org/SDL2/SDL_LockAudioDevice)
and [Web Audio specification](https://www.w3.org/TR/webaudio/). The headless runner
uses Chrome's [wall-clock timeout](https://developer.chrome.com/docs/automation-and-testing/headless-cli#timeout).
