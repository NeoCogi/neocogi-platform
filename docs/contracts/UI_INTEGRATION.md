# UI workspace integration notes

The [UI workspace](https://github.com/NeoCogi/neocogi-ui) owns the frontend
behavior described below. These notes preserve its existing host, input, asset
and painting contracts; they do not add a UI role to renderer resource groups.
A compositor selects its actual framebuffer or image surface at binding time
and creates an ordinary group with depth tests, writes and clears disabled.
See [drawing targets and permissions](RENDER_SURFACES.md) for that contract.

## Embedded input and host updates

An embedded preview leaf declares keyboard bits `focus | Tab stop | embedded
scope` (`1 | 2 | 512`). While focused, ordinary Tab and menu/window navigation
keys reach the leaf instead of the outer manager. Ctrl+Tab, with optional Shift,
leaves the scope through the editor's normal focus traversal. Outer menus and
modal eligibility still take priority. `WidgetProperties` uses the pre-release v1 baseline;
hosts and UI plugins must be rebuilt together.

Logical keyboard identity uses Rust enum payloads: `KeyCode::Character(u32)`
carries a validated Unicode scalar and `KeyCode::Function(u8)` carries 1 through
24; named commands carry no extra value. `KeyStroke` combines that identity with
`KeyAction::Pressed { repeat }` or `KeyAction::Released` and an independent
modifier snapshot (Alt=1, Ctrl=2, Shift=4, Super=8). A repeated release cannot be
constructed. Unknown wire tags reject at the generated COM boundary; invalid
scalar/function values and unknown modifier bits return `NC_E_INVALID_ARG`
before input delivery. Key identity describes shortcuts; committed text still
arrives separately through `UiInput::Text`, including IME and paste.

The shared application host supplies `AppState::after_update` after all editor
event callbacks return and before rendering. Preview compilation, resource
installation, input delivery, and offscreen rendering belong at that boundary.
`AppState::text_input_area` selects the single native IME placement after both
UIs have updated; an embedded host translates preview coordinates into its
pane's committed screen rectangle. Neither operation belongs inside painting.

## Asset and paint lifetime

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
