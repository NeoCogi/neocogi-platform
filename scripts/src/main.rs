//! Cargo-run repository tools. No Python interpreter or shell command evaluation is required.
#![deny(missing_docs)]
// Assign Windows children to their owned job atomically during process creation.
#![cfg_attr(windows, feature(windows_process_extensions_raw_attribute))]

#[path = "../../examples/native/gltf/artwork.rs"]
mod artwork;
mod assets;
mod browser;
mod cargo_artifacts;
mod local_sources;
mod packages;
mod process;
mod shader_perf;
mod source_map;
mod workspace;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use workspace::Workspace;

/// Development commands available through the repository's `cargo tools` alias.
#[derive(Parser)]
#[command(about, version)]
struct Arguments {
    /// Workspace root to use for checkout-dependent operations from outside the workspace.
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,
    /// Operation to perform; commands never upload or publish packages.
    #[command(subcommand)]
    command: Operation,
}

/// Each operation owns its own files and external tool requirements.
#[derive(Subcommand)]
enum Operation {
    /// Qualify every example shader and compare min-size native workloads with perf and driver costs.
    ProfileShaders {
        /// Backend compiled into the one platform library.
        #[arg(long, value_enum, default_value = "vulkan")]
        backend: shader_perf::Backend,
        /// Explicitly include the optional Python frontend and select Python-like Triangle source.
        #[arg(long)]
        shader_python: bool,
        /// Fresh report/deployment directory separate from both Cargo build directories.
        #[arg(long)]
        output: PathBuf,
        /// Optional prior deployment containing the same three applications and lib/nc_platform.
        #[arg(long)]
        reference: Option<PathBuf>,
        /// Rendered frames per timed application invocation; startup is included explicitly.
        #[arg(long, default_value_t = 300)]
        frames: u32,
        /// Alternating repetitions per application and variant, after warmup.
        #[arg(long, default_value_t = 3)]
        runs: u32,
        /// Maximum candidate/reference median elapsed-time ratio when a reference is supplied.
        #[arg(long, default_value_t = 1.2)]
        max_slowdown: f64,
        /// Collect Linux perf task-clock and context-switch counters for each timed run.
        #[arg(long)]
        perf: bool,
        /// Require Intel driver diagnostics, zero spills and at most 4096 instructions per shader.
        #[arg(long)]
        intel_stats: bool,
    },
    /// Fetch pinned glTF/HDR assets, preserving existing files unless checksums pass.
    FetchGltfExamples {
        /// Destination directory; defaults to examples/native/gltf/assets.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Qualify the three real crate archives and an independent generated COM consumer.
    CheckPackages {
        /// Permit uncommitted inputs while developing the package set.
        #[arg(long)]
        allow_dirty: bool,
        /// Resolve and build using only cached Cargo dependencies.
        #[arg(long)]
        offline: bool,
        /// Also execute the extracted consumer with this installed stable toolchain.
        #[arg(long)]
        stable_toolchain: Option<String>,
    },
    /// Build and execute actual DOM/WebGL regressions in headless Chrome/Chromium.
    TestBrowser {
        /// Chrome/Chromium executable, otherwise discovered on PATH.
        #[arg(long)]
        browser: Option<PathBuf>,
        /// wasm-bindgen executable matching the workspace pin, otherwise found on PATH.
        #[arg(long)]
        wasm_bindgen: Option<PathBuf>,
    },
    /// Install wasm-bindgen-cli using the workspace's exact wasm-bindgen version.
    InstallWasmBindgen,
    /// Generate LLDB commands from Cargo's trim-paths sidecar for an executable.
    LldbSourceMap {
        /// Debug executable; the adjacent .trim-paths.json file is read if present.
        executable: PathBuf,
        /// Write an LLDB command file instead of printing commands to stdout.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Print a temporary Cargo patch overlay for a local Platform checkout.
    LocalSources {
        /// Checkout containing the explicit Platform workspace members.
        #[arg(long)]
        platform: PathBuf,
    },
}

/// Dispatch explicitly selected operations and return owned, contextual errors.
fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let workspace = || Workspace::discover(arguments.workspace.as_deref());
    match arguments.command {
        Operation::ProfileShaders {
            backend,
            shader_python,
            output,
            reference,
            frames,
            runs,
            max_slowdown,
            perf,
            intel_stats,
        } => shader_perf::run(
            &workspace()?,
            shader_perf::Options {
                backend,
                shader_python,
                output,
                reference,
                frames,
                runs,
                max_slowdown,
                perf,
                intel_stats,
            },
        ),
        Operation::FetchGltfExamples { output } => {
            let directory = match output {
                Some(directory) => directory,
                None => assets::gltf_download_directory(workspace()?.root()),
            };
            assets::fetch(&directory)
        }
        Operation::CheckPackages { allow_dirty, offline, stable_toolchain } => {
            packages::qualify(&workspace()?, allow_dirty, offline, stable_toolchain.as_deref())
        }
        Operation::TestBrowser { browser, wasm_bindgen } => browser::run(&workspace()?, browser, wasm_bindgen),
        Operation::InstallWasmBindgen => {
            let workspace = workspace()?;
            process::run(
                workspace
                    .cargo("install")
                    .args(["wasm-bindgen-cli", "--version", &workspace.bindgen_version()?, "--locked"]),
            )
        }
        Operation::LldbSourceMap { executable, output } => {
            let commands = source_map::commands(&executable)?;
            if let Some(output) = output {
                std::fs::write(output, commands)?;
            } else {
                print!("{commands}");
            }
            Ok(())
        }
        Operation::LocalSources { platform } => {
            print!("{}", local_sources::overlay(&platform)?);
            Ok(())
        }
    }
}
