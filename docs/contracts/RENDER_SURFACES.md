# Drawing targets and resource groups

`IRenderSurface` selects where a resource group's commands draw: a view of the
window's existing framebuffer or an independently rendered image. Creating either
kind acquires no frame, clears no pixels and presents nothing. Both the window
and its separate graphics host expose the surface and group factories. All
authored declarations retain `#[version(1)]`; this pre-release change replaces
the old constructors without compatibility aliases.

## Select the target before creating a group

Call `IRenderSurfaceFactory::create_surface(RenderSurfaceDesc)`, then
`IRenderClientFactory::create_client(limits, surface, permissions)`. The returned
`IRenderClientDomain` exclusively controls one group; delegate its restricted
`resources()` factory to drawing code. Several groups may select the same surface.
They retain separate ownership, quotas, permissions and closure state; sharing a
target does not share resources.

The generated COM boundary rejects unknown enum tags with `NC_E_INVALID_TAG`.
Known target kinds with invalid dimensions return `NC_E_INVALID_ARG`; oversized
image allocations return `NC_E_OUT_OF_RANGE` before publication.

| Surface kind | Descriptor size | Submission | Depth |
| --- | --- | --- | --- |
| `Framebuffer` | Zero; `size()` follows the window's physical pixel size | Submit recorded blocks in the window renderer's ordered frame | Uses the window's depth attachment |
| `Image` | Bounded nonnegative physical pixel size | Query the surface for `IRenderer` and submit an independent image frame | Color only; depth permissions reject at group creation |

A framebuffer view exposes no independent `IRenderer`. The window renderer owns
frame acquisition, the initial whole-frame clear and presentation. View commands
join its normal order; regional clears affect only the recorded view rectangle.
`resize()` rejects, and `completed_image()` returns `NC_E_NOT_FOUND`. Releasing
the public view does not invalidate groups already created for it.

An image surface accepts blocks from any group bound to that exact surface.
Matching size or device identity does not authorize another image surface to use
them. The window rejects image-target blocks; an image renderer rejects
framebuffer-target blocks. Both check closed groups and selected targets after
foreign stream/resource calls and before native execution or output allocation.
Surface identity is authenticated through the creating device's local registration;
an external implementation cannot substitute matching public metadata.

## Grant operations independently

`RenderClientPermissions` describes operations without assigning a UI or
application role. Disabled operations return `RENDER_E_PERMISSION`.

| Field | Operation permitted |
| --- | --- |
| `clear_color` | Explicit regional color clears |
| `clear_depth` | Explicit regional depth clears |
| `write_color` | Pipelines with a nonzero color write mask |
| `write_depth` | Pipelines that write depth |
| `read_depth` | Pipelines with a depth comparison other than `Always` |
| `write_resources` | Buffer and texture updates; initial creation remains allowed without it |
| `import_images` | The trusted host may import an authenticated immutable image into this group |
| `initialize_depth` | Insert a depth-one clear at the start of each recorded view, within its root rectangle |

`initialize_depth` requires `clear_depth`. All four depth fields require a depth
attachment. Inconsistent permissions return `NC_E_INVALID_ARG` at construction.
Clear and draw-write permissions are independent: allowing a clear does not allow
a pipeline to write that attachment, or conversely.

A UI compositor normally enables color writes, resource updates and image imports,
leaving depth tests, depth writes, depth clears and initialization off:

```rust
let permissions = RenderClientPermissions {
    write_color: true,
    write_resources: true,
    import_images: true,
    ..RenderClientPermissions::none()
};
```

Enable `clear_color` as well if that compositor needs regional clears. These
permissions work for either target kind. A 3D widget can use a separate group on
the same framebuffer with depth enabled. A following UI overlay then leaves the
widget's depth values intact. Permissions allow operations; pipeline settings
still determine which operations a particular draw uses.

The Rust `Default` permission set supports framebuffer drawing, resource updates
and initial regional depth clearing, with imports disabled. Image clients must
choose color-only permissions explicitly. `none()` disables all optional operations.

## Independent image lifetime

Image sizes are physical pixels. Negative or oversized dimensions reject; a zero
component suspends rendering. Resize changes the next requested extent without
modifying completed images. Rendering requires an exact positive size and opaque
frame clear color. The initial whole-image clear is a host operation; a group's
regional-clear permission does not control it.

`completed_image()` returns `NC_E_NOT_FOUND` before the first successful frame.
After success it returns an ordinary immutable `ITexture`, with top-left UVs and
nearest filtering. Retained outputs remain unchanged after later frames, resize
or surface release. Failed rendering preserves the previous completed image.
Sampling an output requires a same-device import into a group with `import_images`
enabled; importing grants no permission to render back into that immutable image.

Image rendering never acquires a native swapchain image, presents or clears the
window backbuffer. Every backend allocates a fresh output per completed frame.
Glow/WebGL use framebuffer-backed RGBA8 textures. WGPU uses color-only RGBA8
attachments and the generic executor, completing error scopes before publication.
Window passes retain their own format and depth attachment.

Vulkan uses a color-only RGBA8 render pass, framebuffer, command pool, command
buffer and completion fence with the same generic executor as window work.
Completed pixels transition to shader-readable layout. Publication currently
waits on the CPU; native acquisition/presentation semaphores do not participate.
Submitted owners are installed before submission and retained if submission or
waiting leaves completion uncertain. Abandoning an unsubmitted frame is recoverable.

## Native image ownership

Uploads return owned initialized images. Output creation returns a single-use
destination consumed by its frame; only successful finish publishes an immutable
image. Failure publishes nothing and permits a fresh attempt. There are no
caller-selected native image keys or explicit destruction calls.

GL/Vulkan output aggregates own their framebuffer and exact image. In-flight or
uncertain work retains that aggregate even for a frame containing only the host's
clear. Final release transfers an owned retirement packet to the driver; its
framebuffer retains its image until attachment destruction. Sampled images and
per-import accounting have their own owners. A retained completed image survives
later surface work without retaining the renderer facade. See
[GPU rendering](GPU_RENDERING.md) for completion and device lifetime rules.

UI-specific input routing, asset publication and painter budgets remain documented
separately in [UI workspace integration](UI_INTEGRATION.md).

## Acceptance checks

Platform acceptance exercises generated COM calls, several groups sharing each
target kind, regional clears, target rejection, immutable outputs and surface
release. Glow also reads native pixels in the platform test. Backend suites read
native image pixels for Glow, WGPU and Vulkan.

```sh
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform-plugin --lib --features test-support -- --include-ignored --test-threads=1
cargo test --locked -p neocogi-render-gpu --features glow,vulkan,wgpu,test-support --lib backends:: -- --include-ignored --test-threads=1
```

The combined backend command needs a working display and adapters; software
Vulkan is sufficient. Run platform acceptance separately with
`--no-default-features --features vulkan,test-support` or `wgpu,test-support` for
those providers. SDL dummy/offscreen drivers cannot create WGPU windows. The
optional Khronos validation layer can be enabled with
`VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation`. Browser execution is checked
separately by `scripts/test_browser.py`.
