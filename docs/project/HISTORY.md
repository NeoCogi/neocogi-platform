# Native builds with ordinary Cargo

Dynamic examples now use two standard Cargo commands: build the shim package's
backend-named library examples, then build/run the application with the same
backend/profile/target. The application's Rust build script stages the exact
prebuilt pair and tracks inputs and deployed outputs for rebuilds. The Python
plugin builder is removed. Missing libraries return a profile-specific Cargo
preparation command before model import or HDR processing. See NATIVE_LAYOUT.md
and the native example README for the current invocation.

# Backend-specific shader artifacts

The shader request now selects WGSL, SPIR-V or GLSL ES300, and success owns only
that representation. Device families choose their immutable format; importers
check target agreement and charge entry plus selected code bytes. The prior
three-output shader request/report ABI is superseded without compatibility
aliases. Rebuild SDK clients and both providers together. Optional compiler output
features enable only required Naga writers; GL keeps its guarded SPIR-V roundtrip
as temporary scratch. Compiler and platform deployment filenames both identify
the selected backend. Current contracts are in SHADER_COMPILATION.md and
GPU_RENDERING.md; older entries below describe earlier behavior.

# Two-package SDK consolidation

The current pre-release layout supersedes the eleven-package release set with
`neocogi-platform` and `neocogi-platform-dylib`. Implementations, shared contracts,
schema generation and stable build-script staging are SDK modules. The final
shim package supplies platform and independent compiler discovery roots, with
backend-suffixed deployed platform filenames. Examples and fixtures remain
unpublished. Older entries below describe the historical architecture and retain
their former names for context; current contracts are in COMPONENTS.md,
NATIVE_LAYOUT.md and RELEASING.md.

# Dependency compatibility and revision history

NeoCogi UI depends on compatible NeoCogi Platform packages. The repositories can
evolve independently; a Platform revision does not require a corresponding UI
commit. Manifests and lockfiles select reproducible dependencies, and integration
tests qualify supported combinations.

UI selects the Platform package release `0.1.0-alpha.2`. That dependency graph
is independent of repository commit pairing. Upgrade related Platform packages
together and avoid resolving shared Rust contract types from different Cargo
sources. See the [platform migration guide](PLATFORM_PLUGIN_MIGRATION.md).
The current manifests pin the published COM, host and IDL packages to `0.1.0`;
these dependency choices do not require synchronizing repository commits.

Static consumers must resolve compatible shared Rust types. Dynamically loaded
plugins must satisfy the COM SDK's interface, ABI and threading checks as well as
the behavior required by their consumers. Matching package names, version labels
or interface IDs alone do not establish compatibility. Qualify upgrades through
the affected static, shared-library and browser integration paths.

## Historical lookup

Historical commit trailers record `NeoCogi-Revision-ID` and the companion
repository. UI trailers also identify the Platform commit used at the time.
Where available, reciprocal links in `refs/notes/neocogi-companions` identify
historical combinations. These notes are optional lookup metadata, not Cargo
build inputs or a requirement for new commits.

```sh
git fetch origin refs/notes/neocogi-companions:refs/notes/neocogi-companions
git log --show-notes=neocogi-companions
```

To reproduce an old build, use that revision's manifests, lockfile and toolchain;
the historical links can help locate the repositories used. Current source
manifests and lockfiles remain authoritative for normal Cargo builds.
The [COM SDK](https://github.com/NeoCogi/neocogi-com) owns COM/IDL documentation.
