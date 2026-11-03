# Native application staging

Final applications select ordinary Cargo cdylib artifacts, then
neocogi-native-layout stages their filenames into app/lib. Only final dylib
packages export plugin discovery. The tool does not run Cargo, resolve plugins,
or choose a renderer at runtime.

neocogi-native-layout-core owns build-only output policy and resource staging.
It has no renderer, COM, math or IDL dependency. Build scripts and tests needing
output policy depend on that package directly; the former
neocogi_render_core::platform::deployment module is removed.

## Output selection

An absolute NEOCOGI_NATIVE_OUTPUT_DIR takes precedence and requires no build
script or OUT_DIR. A relative override rejects. Without an override, automatic
inference requires the consuming package's OUT_DIR, normally supplied by build.rs.
The closest ancestor with Cargo's .cargo-lock marker is the output root.
Missing prerequisites produce a diagnostic naming both supported choices.
No hashed Cargo path, profile name or target triple is reconstructed.

Runtime code receives library filenames only. It joins those names to the
executable-relative lib directory. Build-time absolute paths do not enter that code.

## Exclusive resource slots

Each stage_resources source => destination mapping owns its entire destination
subtree. Its successful replacement contains exactly the source's regular files
and directories, including empty directories. Removed/renamed source members
disappear from the destination. File-to-directory and directory-to-file changes
are legal for an already owned slot.

Destinations are nonempty normalized relative paths. The output root, parent
traversal, lib and .neocogi-* are reserved. Inputs must be regular files or
directories. Check every component of each supplied input path for symlinks before
canonicalizing it, including paths such as `linked/assets`, `linked/file.txt`
and `linked/.`. Destination symlink ancestry also rejects. Pruning an old child
symlink removes that link without following it. This build-input policy assumes
sources are not concurrently replaced; it is not a filesystem sandbox.

One consuming package may rebuild its exact destination slot using a different
source (including A/B/A feature changes). Different slots cannot overlap by prefix,
even across packages or macro invocations. Duplicates in one invocation reject.
A preexisting destination without an ownership receipt rejects rather than
silently acquiring authority to delete it. Use a clean staging output when
migrating from the former additive copier.

The tool serializes ownership checks and publication across compiler processes
using a filesystem lock. Typed versioned receipts contain relative destinations
and an opaque SHA-256 package-owner fingerprint, not private source paths.
Receipts belong to build output, not the runtime ABI or a plugin registry.

Every candidate is prepared before any mapped output changes. Invalid mappings,
missing sources or copy failures publish none of those candidates. Filesystem
publication errors can leave earlier complete mappings installed; staging does
not promise an atomic transaction across all mappings. Each replacement retains
its old complete output until the candidate is ready, attempts restoration on
rename failure, and preserves recovery storage if restoration also fails.

Validate the complete mapping set before creating any temporary candidate.
The parent directory used for each candidate must be outside every directory
input, including inputs from other instructions and empty input directories.
Otherwise reject with a diagnostic naming the destination and conflicting input;
installed outputs and ownership receipts remain unchanged. This prevents both
copying a candidate into another mapping and recursively copying an output
located inside its own source. Candidate directories remain siblings of their
destinations to preserve rename-based replacement on the same filesystem.
An already owned input equal to its destination is allowed when its temporary
sibling is outside that input. Validation does not reject all input/output equality.

Unchanged snapshots preserve deployed file timestamps. Compiler dep-info tracks
source and output files/directories: removing an output or changing its membership
invalidates cached expansion. Removing a mapping entirely requires clean packaging;
a macro that no longer runs cannot retire its old slot automatically.

## Compiler-facing test artifacts

Real macro-consumer tests use the exact library recorded in their own executable's
rustc dependency information. The development workspace enables Cargo's
`binary-dep-depinfo` option and explicitly loads the macro dependency for that
record. Missing, ambiguous or deleted recorded artifacts fail with a rebuild
diagnostic. Directory scans and modification dates never select a substitute.
The fixture invokes the pinned rustc directly; it does not start a nested Cargo
build or retain another Cargo target directory.
