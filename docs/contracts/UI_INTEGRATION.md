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

The platform admits up to 4 MiB of UTF-8 per complete commit. A larger commit
is discarded as a unit. Its position in the input queue returns
`PLATFORM_E_INPUT_LIMIT` (`-1008`) from `window.poll()` without invoking the
input sink. Continue polling to receive subsequent transitions.

Each Platform window route can retain 4,096 ordinary transitions and 8 MiB of owned
queued text. The first overflow reserves one `UiInput::Cancel` after the admitted
prefix. Input arriving before that cancellation is dequeued is discarded on that
route. Native close and quit keep their insertion position in reserved storage.
On cancellation, clear held input and resume admitting new transitions. Pointer
gestures need a fresh down and keyboard repeats need a fresh press of that key.
The Platform resets its wheel state and any active browser composition before
the callback. SDL owns native IME preedit. Complete SDL commits delivered after
recovery are admitted as new standalone transitions.
An attempted cancel is consumed even if the input sink reports failure. The
entry and byte limits cover Platform queues, with one detached transition in delivery.

Keypad arithmetic maps to `KeyCode::Character` with the corresponding `+`, `-`,
`*`, or `/` scalar. An ordinary key has the same identity when the host reports
that operator as the logical character. Keypad Enter maps to `KeyCode::Enter`.
Press, repeat, release and the current modifier snapshot are preserved. Behavior
for NumLock-dependent digits and navigation and locale-specific decimal keys is
unspecified.

SDL2 reports its layout keycode and the modifier snapshot separately. A browser's
`KeyboardEvent.key` can already incorporate Shift. For example, SDL2 can report
`=` with Shift where the browser reports `+` with Shift. Portable shortcuts
can handle this difference while SDL2 remains the native input backend.

The shared application host supplies `AppState::after_update` after all editor
event callbacks return and before rendering. Preview compilation, resource
installation, input delivery, and offscreen rendering belong at that boundary.
`AppState::text_input_area` selects the single native IME placement after both
UIs have updated; an embedded host translates preview coordinates into its
pane's committed screen rectangle. Neither operation belongs inside painting.

A retained clipboard keeps its host service available according to the active
driver's lifetime rules. SDL on Windows uses a private hidden owner for clipboard
writes. It has no renderer or application input route. Browser access
follows the page's clipboard permissions. A successful read can contain empty
text. When SDL reports a read failure, including a selection timeout, the
failed receipt carries empty text.

On Linux with SDL's X11 driver, a successful write confirms X11 selection
ownership and a live responder holding the complete text on SDL's actual
display. The responder serves paste requests while a clipboard projection is
retained, including after release of the application window and its SDL event
pump. It serves a selection until another client replaces it or the service
ends. Persistence
after final release depends on an external clipboard manager. Reads use a
separate requestor connection. Both directions support bounded UTF-8 and X11
incremental transfers up to 1 MiB. A read with a stalled owner or a failed
text conversion delivers a failed receipt. Socket and protocol waits are
bounded. The configured display's non-loopback name resolution and ordinary
Xauthority file reads use the operating system. An expired setup cannot publish
a selection. X11 loads the system `libxcb.so.1` on demand.

With SDL2's Wayland driver, reads use the SDL clipboard while its currently
focused window has a live public Platform input route. The application must
continue polling that route. When the route is unavailable, including after
release of the last public window while graphics retain its native surface,
the read completes with `PLATFORM_E_BACKEND`. SDL2 provides no compositor
ownership acknowledgement for writes. A valid write completes once with
`PLATFORM_E_BACKEND` and empty text, leaving the selection untouched.
An editor keeps the source text when a cut receives that failure.

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
