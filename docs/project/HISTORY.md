# Dependency compatibility and revision history

NeoCogi UI depends on compatible NeoCogi Platform packages. The repositories can
evolve independently; a Platform revision does not require a corresponding UI
commit. Manifests and lockfiles select reproducible dependencies, and integration
tests qualify supported combinations.

UI currently pins its Platform dependencies to a Git commit. That pin selects a
dependency graph, rather than establishing a mandatory pair of repository
revisions. When upgrading the current pre-release packages, update the related
Platform dependencies together and avoid resolving shared Rust contract types
from different Cargo sources. See the [platform migration guide](PLATFORM_PLUGIN_MIGRATION.md).
The current manifests also pin the published COM, host and IDL packages to
`0.1.0-alpha.1`; these are dependency choices, not a requirement to synchronize
repository commits.

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
