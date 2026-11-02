# Companion revisions

NeoCogi UI and NeoCogi Platform use exact companion revisions. UI dependency
declarations pin the platform Git commit; both use the same published COM SDK
source/version selected by their manifests and lockfiles.

Historical commit trailers record `NeoCogi-Revision-ID` and the companion
repository. UI trailers also identify the matching platform commit. Exact
reciprocal links are recorded in `refs/notes/neocogi-companions`; notes can refer
to both immutable commits without requiring circular commit hashes.

```sh
git fetch origin refs/notes/neocogi-companions:refs/notes/neocogi-companions
git log --show-notes=neocogi-companions
```

Use the companion commit identified for the selected revision, including that
revision's dependency versions and toolchain. Current source manifests remain
authoritative for normal Cargo builds. A matching package name alone does not
establish that two different source revisions have compatible declarations.
The [COM SDK](https://github.com/NeoCogi/neocogi-com) owns COM/IDL documentation.
