//! Build-script APIs; enable `build-support` on a build dependency, not runtime code.
mod schema;
pub use schema::{BuildError, BuildOutput, Module, SharedSources, build, sources};
/// Native output policy and exclusively owned resource snapshots.
pub mod layout;
