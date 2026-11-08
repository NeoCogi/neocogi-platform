# Browser platform regression

This final test cdylib owns one COM host and exports run_platform_input_tests.
It calls the production platform's installed DOM listeners and WebGL executor.
An asynchronous export exercises real AudioWorklet playback and fake-microphone
capture, including typed capabilities, bounded queues, denial and late cancellation.
The reusable platform test helper is a Rust function; only this final artifact
owns the JavaScript entry point.

Install a Chrome/Chromium executable and the CLI matching the workspace's pinned
wasm-bindgen version:

~~~sh
cargo tools install-wasm-bindgen
cargo tools test-browser --browser /path/to/chrome
~~~

The runner builds neocogi-browser-tests for wasm32, discovers the actual Cargo
artifact from JSON output, generates web bindings, serves disposable files on
localhost, and runs Chrome with a fresh temporary profile and software WebGL.
Use --wasm-bindgen /path/to/wasm-bindgen for a non-PATH installation. The CLI
version must match the workspace manifest exactly.

The HTML imports bindings inside its error boundary. Module/export failures,
exceptions, rejected promises, a missing result element and a stuck RUNNING state
all reject. A load-event barrier waits for asynchronous assertions with real wall
time, and a browser timeout fails. Generated bindings, server and profile
are removed after execution. The headless test disables Chromium sandboxing only
for its trusted local fixture; it does not reuse a personal browser profile.

Coverage includes canonical signed derivatives on window/texture targets, exact
mip-zero texel reads with adversarial coordinates, sampled and zero-initialized
storage pixels, actual WebGL context loss, DOM composition/paste, keyboard,
fractional wheel, pointer chords/cancellation and listener teardown. Synthetic
pointer capture is stubbed because trusted browser input is required for that
primitive. These checks do not claim interactive OS IME, clipboard permissions,
or every browser/GPU combination.

CI runs the same runner using the executable reported by the
[Chrome setup action](https://github.com/browser-actions/setup-chrome).
Compilation remains an additional check, not a replacement for browser execution.
