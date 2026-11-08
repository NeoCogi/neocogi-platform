//! Cargo-run repository tools. No Python interpreter or shell command evaluation is required.
#![deny(missing_docs)]

mod assets;
mod browser;
mod local_sources;
mod packages;
mod process;
mod source_map;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

/// Development commands available through the repository's `cargo tools` alias.
#[derive(Parser)]
#[command(about, version)]
struct Arguments {
    /// Operation to perform; commands never upload or publish packages.
    #[command(subcommand)]
    command: Operation,
}

/// Each operation owns its own files and external tool requirements.
#[derive(Subcommand)]
enum Operation {
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

/// Repository location comes from this private package, independent of the caller's CWD.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("scripts is a workspace member")
}

/// Read the single authored workspace pin used by both CLI installation and execution.
fn bindgen_version() -> Result<String> {
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(root().join("Cargo.toml"))?)?;
    let version = manifest["workspace"]["dependencies"]["wasm-bindgen"]
        .as_str()
        .context("workspace wasm-bindgen must have an exact string pin")?;
    Ok(version.strip_prefix('=').context("workspace wasm-bindgen must be pinned exactly")?.to_owned())
}

/// Dispatch explicitly selected operations and return owned, contextual errors.
fn main() -> Result<()> {
    match Arguments::parse().command {
        Operation::FetchGltfExamples { output } => assets::fetch(&output.unwrap_or_else(|| root().join("examples/native/gltf/assets"))),
        Operation::CheckPackages { allow_dirty, offline, stable_toolchain } => packages::qualify(allow_dirty, offline, stable_toolchain.as_deref()),
        Operation::TestBrowser { browser, wasm_bindgen } => browser::run(browser, wasm_bindgen),
        Operation::InstallWasmBindgen => process::run(process::cargo(None).args(["install", "wasm-bindgen-cli", "--version", &bindgen_version()?, "--locked"])),
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
