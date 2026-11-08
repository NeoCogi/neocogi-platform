# Repository tools

This directory is the unpublished `neocogi-platform-tools` Cargo package. From
the repository root, `cargo tools` expands to
`cargo run --locked --package neocogi-platform-tools --`. Rust unit and CLI tests run with
`cargo test -p neocogi-platform-tools`.

| Command | Behavior |
| --- | --- |
| `cargo tools fetch-gltf-examples [--output DIR]` | Download the pinned models/HDR over Rust HTTPS, verify SHA-256, then atomically install each file. Verified cached files need no network. |
| `cargo tools check-packages [--offline] [--allow-dirty] [--stable-toolchain stable]` | Construct and inspect the three release archives, extract outside the checkout, and build/test them with an independent generated COM consumer. Never publishes. |
| `cargo tools install-wasm-bindgen` | Install `wasm-bindgen-cli` at the exact workspace pin using Cargo. |
| `cargo tools test-browser [--browser PATH] [--wasm-bindgen PATH]` | Build the real Wasm fixture, generate web bindings and execute DOM/WebGL/audio tests in headless Chrome/Chromium with fake microphone devices. |
| `cargo tools lldb-source-map EXECUTABLE [--output FILE]` | Translate Cargo's adjacent `.trim-paths.json` into native LLDB commands; absent sidecars produce no commands. |
| `cargo tools local-sources --platform CHECKOUT` | Print a temporary Cargo patch overlay for published libraries without modifying manifests, lockfiles or global configuration. |

Tools require Cargo and Rust, without Python, curl, shell command evaluation or
external tar/gzip programs. Browser execution additionally requires Chrome or
Chromium and the matching wasm-bindgen CLI. Package builds retain their ordinary
native prerequisites, such as SDL2 development libraries. Downloads are explicit;
application builds and build scripts never invoke these tools.

Package inspection rejects paths outside the versioned root, links, duplicate
members, missing regular manifests/notices, nonregistry dependencies and duplicate
schema ownership before extraction. Only contracts, SDK and final shims are
release packages; this tools package and application/test fixtures stay private.
Qualification uses one target directory sequentially, removes its temporary
workspace, and tests staging restoration plus propagation of changed input bytes.

The browser runner requires exactly one `<pre id="result">PASS</pre>` in the
captured DOM. Missing/stuck/failed or duplicate results, mismatched binding tools,
failed processes and a 90-second browser deadline all fail. Its loopback server,
generated bindings and fresh browser profile are disposable and shut down on
success or failure. A load-event completion barrier uses real wall time so capture
and worklet processing finish before DOM capture; an 80-second capture timeout
precedes the 90-second process deadline.

That deadline covers spawning, execution and collection of both diagnostic
streams. Output spools concurrently to temporary regular files, so inherited
handles never require waiting for pipe EOF or joining reader threads. The tool
terminates its Unix process group or Windows job before returning, including
when the direct child exits normally while descendants remain. Unix polling
keeps the leader unreaped until group termination to prevent PID reuse; Windows
assigns the child to its job atomically at creation. Windows tooling uses the
workspace's pinned nightly Rust process-attribute API for this assignment. This
private tooling requirement does not change the published SDK's stable support.

For trimmed native debugging, generate a command file after building:

```sh
cargo tools lldb-source-map target/debug/triangle --output /tmp/neocogi-source-map.lldb
lldb target/debug/triangle
```

Then run `command source /tmp/neocogi-source-map.lldb` in LLDB after selecting the
target, or place that command in CodeLLDB's `preRunCommands`. Explicit debugger
maps retain precedence because generated mappings append. Paths are quoted as
LLDB arguments; control characters and unsupported/corrupt metadata reject
before output. This replaces the former Python debugger import hook.

`package-consumer/` is an independent qualification fixture, copied outside the
checkout before Cargo is invoked there. `com_dependencies.rs` remains a Rust
source helper for repository fixtures.
