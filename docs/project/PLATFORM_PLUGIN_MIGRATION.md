# GPU consolidation and platform plugin migration

The GPU implementation now owns the concrete backend modules. The selected
assembly owns SDL/browser hosting as well as graphics, so its package and native
artifact names now describe the platform responsibility. This is a pre-release
source/catalog migration; former packages, paths and identities have no aliases.

| Former name | Current owner/name |
| --- | --- |
| `neocogi-render-glow` | `neocogi-render-gpu`, feature `glow`, module `backends::glow` |
| `neocogi-render-vulkan` | `neocogi-render-gpu`, feature `vulkan`, module `backends::vulkan` |
| `neocogi-render-wgpu` | `neocogi-render-gpu`, feature `wgpu`, module `backends::wgpu` |
| `neocogi-render-webgl` | `neocogi-render-gpu`, feature `webgl`, module `backends::webgl` |
| `neocogi-render-plugin` | `neocogi-platform-plugin` |
| `neocogi-render-plugin-dylib` | `neocogi-platform-plugin-dylib` |
| Rust library/native artifact `nc_render` | `nc_platform` |
| `crates/render/plugin` | `crates/platform/plugin` |
| Catalog `neocogi.render.<backend>` | `neocogi.platform.<backend>` |

The platform's Cargo backend flags are unchanged: default `glow`, or
`--no-default-features --features vulkan`, `wgpu`, or browser `webgl`. Exactly
one backend is required by platform assembly. GPU implementation features are
additive, with empty defaults: multiple typed families may compile together, and
CPU-only consumers need no graphics libraries. Merging the crates does not add
runtime renderer selection. SDL/browser window and input assembly stays in the
platform plugin, outside the generic GPU recording/admission engine.

Update static imports from `nc_render::PlatformObject` and
`nc_render::plugin_descriptor()` to `nc_platform`. Static root acquisition can
derive the identity from `nc_platform::plugin_descriptor().plugin_id`, avoiding a
second backend-name table. Dynamically loaded descriptors expose the same field.
The generic discovery export remains **`nc_get_plugin_descriptor_v1`**. Public
platform/renderer interfaces, method signatures and interface IDs remain owned by
`neocogi-render-core`; this change introduces no wire ABI revision.

Cargo artifact consumers should rename their dependency alias and generated
environment-variable use together. For an alias `platform-library` with
`artifact = "cdylib"`, the staging macro input is:

```rust
neocogi_native_layout::stage_library!("CARGO_CDYLIB_FILE_PLATFORM_LIBRARY_nc_platform")
```

Its package is `neocogi-platform-plugin-dylib`, and its path is
`crates/platform/plugin/dylib`. Update package manifests, feature forwarding,
staged library inventories, browser binding inputs, discovery expectations,
tests and scripts together. Retain the shader compiler dependency and load or
register it before acquiring the platform root. Activation still validates its
exact compiler contract before initializing SDL or creating any device.

Companion NeoCogi UI pins a platform Git revision. Updating its revision is an
explicit companion upgrade: select a revision containing this migration and
update **all** its platform-owned packages together, including core, GPU, support,
shader and native-layout dependencies. Mixing a renamed local implementation
with core/compiler packages from a different source can create duplicate Rust
binding types even when their COM interface IDs match. Existing old-revision UI
checkouts continue to describe their pinned graph; they are not compatibility
consumers of the new source. The same rule applies to static and Cargo-artifact
dynamic compositions.

Fresh qualification should include CPU-only GPU tests, selected dependency graphs,
all native backend suites, platform activation and single-backend diagnostics,
static examples, real final-library discovery/acquisition, and the browser build.
Native WGPU allocation retirement qualification uses `wgpu,test-support`; its
counter feature stays disabled in ordinary WGPU consumers.

In a companion checkout whose platform dependencies and lockfile have been
migrated together, the consumer checks are:

```sh
cargo test -p neocogi-demo-host --features monolith-gfx,glow
cargo test -p neocogi-demo-plugins --test shared_runtime --features glow
cargo run -p neocogi-demo-smoke --no-default-features --features dynamic,glow,embed-assets -- --smoke
cargo check -p neocogi-demo-full --target wasm32-unknown-unknown --no-default-features --features webgl,embed-assets
```

The shared-runtime test inspects the real Cargo-built `nc_platform` library's
discovery export, checks its catalog identity and verifies host/plugin registry
sharing after relocation. The dynamic smoke application then acquires the actual
platform root, creates a window and exercises GPU/UI/resource behavior through
COM. A static host test alone does not qualify the dynamic boundary.

## Build-only output policy and resource ownership

Use neocogi-native-layout-core::output_root(Some(anchor), explicit) in build
scripts or test tooling. Add that build-only package directly instead of importing
the removed neocogi_render_core::platform::deployment module. Absolute explicit
output needs no anchor/build.rs; default inference requires the consumer OUT_DIR.

Resource mappings now own exact nonoverlapping destination subtrees. Use a clean
staging directory when migrating from additive copying; unclaimed existing output
rejects instead of being deleted. Removed members are pruned only inside an owned
slot. See [native layout contracts](../contracts/NATIVE_LAYOUT.md).

## Native example applications

The provider no longer owns executable examples or glTF import dependencies.
Use neocogi-platform-examples with --bin triangle, gltf_damaged_helmet or
gltf_brainstem. Default features select static,glow. Alternate builds use
--no-default-features --features static,vulkan or dynamic,vulkan (and similarly
glow/wgpu). No old example forwarding entry points are retained.

The dynamic composition uses normal Cargo cdylib artifacts and executable-relative
loading; it does not add provider Rust implementation dependencies to the executable.
See [native assembly](../../examples/native/README.md) for qualification and relocation.
