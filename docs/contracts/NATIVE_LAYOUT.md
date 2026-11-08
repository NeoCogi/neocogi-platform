# Native application staging

`neocogi-platform-dylib` supplies one platform shared library per backend, including
its renderer-owned shader compiler. Build the selected library
with `cargo build -p neocogi-platform-dylib --no-default-features --features glow --examples`
(or vulkan/wgpu). Cargo selects the backend-named platform library example through their
required features. The application build script stages every known backend library
present in the same Cargo profile's `examples/` directory. It never invokes Cargo, and dynamic
executables have no provider dependency.
The library stages as `libnc_platform_{backend}.so` (target-specific suffix/prefix
on macOS/Windows). No independent compiler library is deployed or loaded.
Use the same profile/target/output as the application. Dynamic applications build
with only their `dynamic` feature; backend features belong to static implementations.
At startup an explicit `--backend NAME` selects exactly that library. Without the
option, one deployed library is selected automatically; none produces preparation
instructions, and several require an explicit choice. Selection never probes or
loads unchosen libraries, never silently chooses another on activation failure,
and validates the loaded descriptor identity against the requested backend. The compiler
contains only that backend's required output support; formats never fall back to
another representation. Missing libraries
remain errors; there is no alternative backend fallback.

Resource staging and output policy live in `neocogi_platform_contracts::build::layout`,
available through the contracts crate's `build-support` feature on a build dependency.
`stage_resources(manifest, root, &[Resource { source, destination }])` returns
`StagedResources { inputs, outputs }`. Build scripts must emit
`cargo::rerun-if-changed` for both sets and track relevant environment overrides
with `cargo::rerun-if-env-changed`. SDK consumers generate normal Rust code;
there are no procedural macros, tracked macro paths or artifact dependencies.

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
even across packages or staging invocations. Duplicates in one invocation reject.
A preexisting destination without an ownership receipt rejects rather than
silently acquiring authority to delete it. Use a clean staging output when
migrating from the former additive copier.

The tool serializes ownership checks and publication across build-script processes
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

Unchanged snapshots preserve deployed file timestamps. Cargo rerun directives
track source and output files/directories. Removing outputs or changing input
membership reruns the build script and restores the exact snapshot. Removing a
mapping entirely requires clean packaging; a build script that no longer invokes
that mapping cannot retire its old slot automatically.

File equality compares size/permissions and streams bytes through fixed 8 KiB
buffers per input. New candidates copy directly; unchanged outputs keep their
timestamps. The no-concurrent-source-mutation and per-mapping publication rules
still apply. The SDK retains the filesystem/resource regressions, and extracted
package qualification exercises a real consumer build script on stable Rust.

## Missing development artifacts

Dynamic examples preflight the executable-relative platform library path before loading
its provider. Missing/inaccessible files return an owned `RenderError` with
`NC_E_LOAD_FAILED`, the actual missing selected path and a backend/profile preparation command.
Automatic discovery with no deployed backend lists a library preparation command;
an ambiguous deployment names the available choices and runtime option.
The build-script hint records only Cargo's profile name, without embedding private
build paths. `debug` maps to Cargo's `dev` profile. For min-size Vulkan, run
`cargo build --profile min-size -p neocogi-platform-dylib --no-default-features --features vulkan --examples` before the
matching `cargo run --profile min-size ... --features dynamic` invocation.
Both commands default to dev and must select the same profile/target. The build
script tracks both prebuilt inputs and staged outputs; rebuilding a shim refreshes
its deployed copy, and deleting a staged output causes restoration on the next
application build. Missing inputs do not prevent compilation or help; acquisition
returns the preparation error. `NEOCOGI_NATIVE_OUTPUT_DIR` redirects staging through
the existing output policy. No extra executable or Python builder is required.
Preflight does not promise an atomic filesystem snapshot or replace the SDK's
fail-fatal policy for an existing corrupt/incompatible library. glTF startup does
this deployment work before model import or HDR preparation; help still needs
neither libraries nor a graphics device.
