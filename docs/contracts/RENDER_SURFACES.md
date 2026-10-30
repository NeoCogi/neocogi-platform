# Independent UI render surfaces

`IRenderSurfaceFactory` creates a two-dimensional offscreen session on the
factory's existing GPU device. Query the returned `IRenderSurface` for
`IRenderer` and pass that projection to the preview UI's render calls. Bind the
window's separate graphics host to each preview runtime before painting. Each
runtime's GFX compositor owns its prepared atlas; asset installation takes only the
candidate bundle. Creating windows and surfaces needs no initial image.

The surface size is in physical pixels. Negative or oversized values fail;
a zero component suspends frame submission. Resize changes the next requested
extent and does not modify completed images. Rendering requires an exact,
positive size and opaque clear color. Surfaces accept only sealed blocks from a
construction-authenticated private UI domain. Public client-domain work is rejected
before output allocation, even when it uses the same generic command vocabulary.

`completed_image` returns `NC_E_NOT_FOUND` before the first successful frame.
After success it returns a normal immutable `ITexture`, with the same top-left
UV convention and nearest filtering as uploaded UI textures. Draw it using the
existing painter texture operation. It is accepted only by its owning device.
Every retained output remains unchanged after later frames, resize, or surface
release. A failed frame leaves the previous completed image available.

Surface rendering never presents, acquires a native swapchain image, or clears
the native backbuffer. A window frame separately presents the editor and draws
the completed preview texture. GPU allocations retire through the existing
device ownership path after all public images and active submissions release
them. Stream reads and resource authentication finish before GPU execution.

Glow and WebGL use framebuffer-backed textures and their common generic executor.
WGPU uses RGBA8 render attachments without depth and its ordinary generic executor.
Every backend allocates a fresh output for each completed frame, so later frames
cannot overwrite published snapshots. Texture passes never acquire or reconfigure a
native surface. Window passes retain their own target format and depth attachment.
WGPU error scopes finish before publication and queue completion retains native owners.

Vulkan uses a private RGBA8 render pass, framebuffer, command pool, command buffer
and completion fence, with the same generic pipeline/binding executor as window work.
The shared native pass builder transitions completed pixels to shader-readable layout.
The first implementation waits for that submission on the CPU before publication;
this makes ownership explicit with a synchronization cost. Native acquisition and
presentation semaphores do not participate. Submitted owners are installed before
queue submission and retained if submission or waiting leaves completion uncertain.
Dropping a frame before submission is recoverable. Native image initialization also
uses the generic executor's completion-owned upload path.

Native acceptance can be run with:

```sh
SDL_VIDEODRIVER=offscreen cargo test -p neocogi-render-plugin --features neocogi-com/runtime --lib -- --ignored --test-threads=1
```

The surface test checks asymmetric source orientation, old snapshot pixels after
later frames, independent compositor atlas identity, native allocation failure,
suspended/resized output, surface release, and final device retirement. WebGL
compilation is checked separately; compilation does not establish browser runtime
coverage.

An embedded preview leaf declares keyboard bits `focus | Tab stop | embedded
scope` (`1 | 2 | 512`). While focused, ordinary Tab and menu/window navigation
keys reach the leaf instead of the outer manager. Ctrl+Tab, with optional Shift,
leaves the scope through the editor's normal focus traversal. Outer menus and
modal eligibility still take priority. `WidgetProperties` is now version 2;
hosts and UI plugins must be rebuilt together.

The shared application host supplies `AppState::after_update` after all editor
event callbacks return and before rendering. Preview compilation, resource
installation, input delivery, and offscreen rendering belong at that boundary.
`AppState::text_input_area` selects the single native IME placement after both
UIs have updated; an embedded host translates preview coordinates into its
pane's committed screen rectangle. Neither operation belongs inside painting.

WGPU GPU checks require a native adapter (software Vulkan is sufficient) and a
working SDL native display for the surface test:

```sh
cargo test -p neocogi-render-wgpu -- --ignored --test-threads=1
cargo test -p neocogi-render-plugin --lib --no-default-features --features wgpu -- --include-ignored --test-threads=1
```

The WGPU test reads actual rendered pixels, verifies top-left source orientation
and retained-output immutability across different-size frames and allocation
failure, then submits an ordinary native frame. Unsupported SDL dummy/offscreen
video drivers return a construction diagnostic instead of reaching SDL's
panicking raw-window-handle adapter.

Vulkan checks exercise actual readback and recovery with a native display:

```sh
VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation cargo test -p neocogi-render-vulkan -- --include-ignored --test-threads=1
cargo test -p neocogi-render-plugin --lib --no-default-features --features vulkan -- --include-ignored --test-threads=1
```

The Khronos validation layer is optional and must be installed to enable it.
The tests cover sampled-image orientation, immutable retained outputs, changed
output extents, rejected allocation, pre-submission abandonment, and subsequent
ordinary window rendering. All three native backends now expose surfaces;
WebGL execution is validated separately from native Glow.

Appearance publication retains the immutable `IAssetBundle::image()` identity.
Each GFX session therefore keeps its own atlas allocation through color,
metric, tint, and slicing edits. A resource rebuild supplies a new image and causes
one upload per affected session. The surface regression counts actual source reads
through repeated preparation, rendering, failed frames, and resizing.

The demo host polls input even when the drawable size is zero. Its native loop waits
16 milliseconds between suspended iterations because this path does not present a
frame and therefore receives no swapchain pacing. Resize and close requests are
checked on the next iteration. Browser hosts use animation callbacks for both
active and suspended iterations. `NEOCOGI_DEMO_FRAMES` counts only successfully
rendered frames; suspension does not consume the limit.

Foreign UI painting and GFX compositor intake have independent frame budgets: at
most 65,536 calls, 64 MiB of input bytes, and 16 Mi units of primitive work. Calls
that clip away still count. Checks precede retained copies and text/vertex scans;
polygon recording accepts at most 4,096 points and charges the square of its point
count to bound ear-clipping work. These budgets reset with the frame, not with
individual widget paint scopes. Exceeding a limit returns `NC_E_OUT_OF_RANGE`.
Final clipped vertex/batch limits remain separate.

Within an active frame, GFX draw errors preserve complete vertex/batch coverage
and keep that frame active. A caller can continue recording or submit earlier
completed work, subject to the remaining frame budget, or call `cancel()`. Multi-primitive draws
can retain complete earlier primitives if a later primitive fails; this does
not promise whole-operation rollback. Attempted calls retain their budget charges.
`begin()` returns `NC_E_BUSY` until the active frame ends through `submit()` or
`cancel()`. Both ending operations release retained resources; the next valid
`begin()` resets frame contents and budget while reusing buffer capacity.

For a texture quad, CPU vertex and batch capacity are reserved before either
recorded length changes. Failure returns `NC_E_OUT_OF_MEMORY`, appends neither
the quad nor its batch, and releases the temporary texture reference. A successful
capacity increase can survive the other reservation's failure. The texture already
exists; these reservations do not allocate GPU storage. Ordinary UI replay cancels
on every draw or submission error and propagates the original status.

Solid replay stages at most 256 triangles on the UI stack and converts one
triangle at a time in GFX, replacing full-range retained conversion buffers.
These intake checks cover the COM drawing boundaries; internal manager chrome
recording still uses its existing infallible helper API.
