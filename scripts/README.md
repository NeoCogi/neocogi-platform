# Repository tools

This directory is the unpublished `neocogi-platform-tools` Cargo package. From
the repository root, `cargo tools` expands to
`cargo run --locked --package neocogi-platform-tools --`. Rust unit and CLI tests run with
`cargo test -p neocogi-platform-tools`.

| Command | Behavior |
| --- | --- |
| `cargo tools profile-shaders --backend vulkan --output DIR [--reference DIR] [--perf] [--intel-stats]` | Validate every production shader and aggregate workload, build the normal min-size library and applications, then measure repeated fixed-frame native runs. Retains artifacts, raw logs and a JSON report outside Cargo targets. |
| `cargo tools fetch-gltf-examples [--output DIR]` | Download the pinned models/HDR over Rust HTTPS, verify SHA-256, then atomically install each file. Verified cached files need no network. |
| `cargo tools check-packages [--offline] [--allow-dirty] [--stable-toolchain stable]` | Construct and inspect the three release archives, extract outside the checkout, and build/test them with an independent generated COM consumer, including a normally built and dynamically loaded platform library. Never publishes. |
| `cargo tools install-wasm-bindgen` | Install `wasm-bindgen-cli` at the exact workspace pin using Cargo. |
| `cargo tools test-browser [--browser PATH] [--wasm-bindgen PATH]` | Build the real Wasm fixture, generate web bindings and execute DOM/WebGL/audio tests in headless Chrome/Chromium with fake microphone devices. |
| `cargo tools lldb-source-map EXECUTABLE [--output FILE]` | Translate Cargo's adjacent `.trim-paths.json` into native LLDB commands; absent sidecars produce no commands. |
| `cargo tools local-sources --platform CHECKOUT` | Print a temporary Cargo patch overlay for published libraries without modifying manifests, lockfiles or global configuration. |

Tools require Cargo and Rust, without Python, curl, shell command evaluation or
external tar/gzip programs. Browser execution additionally requires Chrome or
Chromium and the matching wasm-bindgen CLI. Package builds retain their ordinary
native prerequisites, such as SDL2 development libraries. Downloads are explicit;
application builds and build scripts never invoke these tools.

Shader qualification requires fetched glTF assets, `glslangValidator`, `spirv-val`
and a working native display/GPU. Choose `vulkan`, `glow` or `wgpu`; each invocation
validates all three shader encodings before building only the selected library.
The default is three 300-frame samples per application, with a three-frame warmup
and fixed glTF animation time 1. Measurements include process startup/teardown;
they are not GPU-only frame times. No builds run during timed workloads.

```sh
cargo tools fetch-gltf-examples
cargo tools profile-shaders --backend vulkan --output /tmp/shaders-current \
  --reference /tmp/shaders-previous/candidate --perf --intel-stats
```

The optional reference must be a trusted deployment of the same three programs,
including its matching `lib/libnc_platform.so` and assets. Runs alternate reference
and candidate. A candidate/reference median elapsed-time ratio above 1.2 fails;
`--max-slowdown`, `--frames` and `--runs` set explicit qualification conditions.
The report directory must be new or empty and outside Cargo targets. Copy its
`candidate/` deployment before cleaning if it will be the next reference. Logs,
all emitted corpus artifacts, library byte count and individual samples survive
`cargo clean`.

`--perf` requires Linux perf permissions and records task-clock/context switches
for each timed run. `--intel-stats` requires an Intel Mesa driver providing
`INTEL_DEBUG=vs,fs` diagnostics; a separate uncached run checks every reported SIMD
variant for zero spills/fills and at most 4096 native instructions. Missing driver
diagnostics fail this option. These are qualification gates for the shipped
workloads, not universal limits on accepted user shaders. The complete Vulkan
library must remain below 524288 bytes. Other vendors should use their native
shader profiler and retain equivalent evidence.

Package inspection rejects paths outside the versioned root, links, duplicate
members, missing regular manifests/notices, nonregistry dependencies and duplicate
schema ownership before extraction. Only contracts, SDK and final shims are
release packages; this tools package and application/test fixtures stay private.
Qualification uses one target directory sequentially, removes its temporary
workspace, and tests staging restoration plus propagation of changed input bytes.
The shim is resolved as a nonmember registry dependency, built as a normal
library, then loaded through its real COM export by the independent consumer.
The stable option builds and runs both the library and caller on stable.

The browser runner requires exactly one `<pre id="result">PASS</pre>` in the
captured DOM. Missing/stuck/failed or duplicate results, mismatched binding tools,
failed processes and a 90-second browser deadline all fail. Its loopback server,
generated bindings and fresh browser profile are disposable and shut down on
success or failure. A load-event completion barrier uses real wall time so capture
and worklet processing finish before DOM capture; an 80-second capture timeout
precedes the 90-second process deadline.
The worker that serves files is joined before fixture files are removed.
`tiny_http` closes its accept listener asynchronously; shutdown regressions wait
for actual connection refusal under a finite deadline. They cover idle listeners,
served assets and the asynchronous completion barrier.

That deadline covers spawning, execution and collection of both diagnostic
streams. Output spools concurrently to temporary regular files, so inherited
handles never require waiting for pipe EOF or joining reader threads. The tool
terminates its Unix process group or Windows job before returning, including
when the direct child exits normally while descendants remain. Unix polling
keeps the leader unreaped until group termination to prevent PID reuse; Windows
assigns the child to its job atomically at creation. Windows tooling uses the
workspace's nightly Rust process-attribute API for this assignment. This
private tooling requirement does not change the published SDK's stable support.
On macOS, an unreaped zombie-only group can report `EPERM` when signalled.
The tool accepts that result only after inspecting the group through libproc,
finding its owned leader and confirming all remaining members have exited.
Live members, denied inspection and incomplete snapshots remain failures.

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

## Using local sources in a consumer workspace

Use an overlay in a consumer that already targets the current contracts, SDK and
shim package set. Run the following from that consumer's checkout, pointing at
the Platform checkout whose sources should replace the registry packages:

```sh
cargo run --locked --manifest-path /path/to/neocogi-platform/scripts/Cargo.toml -- local-sources \
  --platform /path/to/neocogi-platform > /tmp/neocogi-local.toml
cargo check --offline --config /tmp/neocogi-local.toml --workspace --all-targets
```

Overlay generation leaves manifests, lockfiles and global configuration alone.
Cargo resolution may refresh the consumer's local lockfile while applying
patches; those development resolutions should not replace its published-source
lockfile. Omit `--offline` when missing ordinary dependencies need downloading.

Tests that launch Cargo need the overlay in inherited configuration: a top-level
`--config` argument is not forwarded to their child processes. For a consumer
using the ordinary registry cache, an isolated Cargo home can hold the temporary
configuration while sharing that cache:

```sh
neocogi_cargo_dir=$(mktemp -d)
cp /tmp/neocogi-local.toml "$neocogi_cargo_dir/config.toml"
ln -s "${CARGO_HOME:-$HOME/.cargo}/registry" "$neocogi_cargo_dir/registry"
CARGO_HOME="$neocogi_cargo_dir" CARGO_NET_OFFLINE=true cargo test --workspace
```

Run the consumer's appropriate test selection and retain the isolated directory
until all child Cargo processes finish. This is a local integration workflow;
release qualification uses the [extracted package consumer](../docs/project/RELEASING.md).
