# Compile-time artifact and resource staging

Procedural macros that copy Cargo-selected shared libraries and exclusively owned resource snapshots into an application layout.

This crate requires nightly Rust for tracked procedural-macro environment and path inputs. Shared-library artifact dependencies also require Cargo’s nightly artifact-dependency support. `NEOCOGI_NATIVE_OUTPUT_DIR` may select an explicit absolute output directory; otherwise a build script’s OUT_DIR anchors inference. Mappings own their whole destination and must not overlap. No nested Cargo build is started.

This is NeoCogi Platform `0.1.0-alpha.2`; authored interfaces remain version 1.
See the [repository documentation](https://github.com/NeoCogi/neocogi-platform/tree/master/docs)
for ownership, supported operations and deployment. The package LICENSE contains
the project license and bundled workspace license notices.
