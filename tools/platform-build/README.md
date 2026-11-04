# Packaged interface sources and build helpers

Declare this crate as a build dependency. It owns the renderer, platform and shader NCIDL declarations and generates bindings into each consumer’s own OUT_DIR.

```toml
[build-dependencies]
neocogi-platform-build = "=0.1.0-alpha.2"
```

```rust
use neocogi_platform_build::{Module, build, sources};
fn main() {
    sources(&[Module::Shader]).and_then(build).expect("generate shader bindings");
}
```

Renderer and platform callers select `Module::Renderer` and `Module::Platform`.
Unselected reachable modules use their ordinary Rust library bindings. Extend
`sources(&[])?` with your own source roots/modules when defining new interfaces.
The helper embeds its canonical sources and reconstructs derived inputs in each
consumer's OUT_DIR, so cached builds need no previous source-folder path.
This build-only dependency owns all shared authored sources; runtime contract
libraries expose generated bindings and do not provide `SCHEMA_ROOT` aliases.


This is NeoCogi Platform `0.1.0-alpha.2`; authored interfaces remain version 1.
See the [repository documentation](https://github.com/NeoCogi/neocogi-platform/tree/master/docs)
for ownership, supported operations and deployment. The package LICENSE contains
the project license and bundled workspace license notices.
