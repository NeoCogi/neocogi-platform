# Preparing and publishing Platform packages

The current release target is **0.1.0-alpha.1**. All workspace packages inherit
that version. The eleven library/build/shared-library packages are enabled for
crates.io; the native examples, example plugin composition and browser test
harness remain unpublished. Publication is a separate explicit action after
local qualification and review.

## Build inputs and archive contents

`neocogi-platform-build` contains the authoritative renderer, platform and shader
interface files and their shared generation helper. Each contract/compiler build
script declares it as a build dependency, generates into its own OUT_DIR and
imports ordinary Rust values from the appropriate contract library. No generation
step searches sibling repository folders. Source declarations stay version 1.

Internal package dependencies specify an exact `=0.1.0-alpha.1` requirement in
addition to their development path. Cargo removes development paths from the
published manifests. This release set is intentionally qualified together;
NeoCogi UI does not need a matching repository commit and must migrate its own
selected contracts separately.

Each published package explicitly includes its sources, applicable build script,
tests, README and a LICENSE containing the project license plus the bundled
workspace notices. Shared interface files occur only in the build-support
package. Example models, review notes and repository-local caches are excluded.
The runtime libraries do not depend on the build helper at runtime.

## Local qualification

Use the checked-in nightly toolchain for the workspace and native staging macros.
`neocogi-native-layout` uses tracked procedural-macro inputs and requires nightly;
shared-library Cargo artifact dependencies also require nightly support. Normal
library consumers do not inherit this repository's nightly-only Cargo settings.
Native platform builds require SDL2 development libraries; Vulkan additionally
requires its loader. Backend selection remains explicit and mutually exclusive
in each platform provider.

Create all interdependent archives together so Cargo can resolve this release's
unpublished members during local packaging:

```sh
cargo package --locked --offline --workspace \
  --exclude neocogi-platform-examples \
  --exclude neocogi-example-plugins \
  --exclude neocogi-browser-tests
```

For development checks of uncommitted changes, add `--allow-dirty`; release
qualification uses a clean tracked tree. Omitting `--no-verify` makes Cargo build
the archived packages. This workspace check is followed by extraction and an
independent consumer build outside the checkout, because Cargo otherwise still
finds repository-level configuration around its verification directory.

Do not publish a package merely because `--list` or `--no-verify` succeeded.
Inspect the actual archives and build their contents using declared dependencies.
Run the normal suites and relevant native/browser checks for runtime changes.
Keep review notes and OPEN_ITEMS.md ignored and uncommitted.

## Publication order

Publish after the selected archives and external consumer have passed and the
review findings have been triaged. The already published COM/host/IDL dependencies
remain pinned at `0.1.0-alpha.1`. Publish this repository's packages in this order,
waiting for each required dependency to become available in the crates.io index:

1. `neocogi-platform-build`
2. `neocogi-native-layout-core`
3. `neocogi-native-layout`
4. `neocogi-render-core`
5. `neocogi-shader-core`
6. `neocogi-shader-compiler`
7. `neocogi-shader-compiler-dylib`
8. `neocogi-render-gpu`
9. `neocogi-render-support`
10. `neocogi-platform-plugin`
11. `neocogi-platform-plugin-dylib`

The two final dylib packages own discovery exports and produce deployable shared
libraries. Reusable providers stay ordinary Rust libraries. Keep the examples
workspace-only; they exercise both linked and loaded compositions.
