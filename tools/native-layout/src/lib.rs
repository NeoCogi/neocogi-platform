//! Copies only Cargo-selected artifacts and authored resources into a native layout.
//!
//! There is no Cargo subprocess, package resolver or generated dependency graph.
//! Macros track both inputs and copied outputs in compiler dep-info: missing or
//! overwritten deployed files invalidate cached compilation, including A/B/A
//! backend switches. Generated runtime code contains only library filenames.
#![feature(proc_macro_tracked_path, proc_macro_tracked_env)]
#![deny(missing_docs)]

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};
use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    LitStr, Token,
};

mod profile;

/// Converts one build/staging failure into a focused compiler error.
fn diagnostic(error: impl std::fmt::Display) -> TokenStream {
    syn::Error::new(proc_macro2::Span::call_site(), error.to_string()).into_compile_error().into()
}

/// Reads and tracks a Cargo compiler environment value without emitting it into code.
fn environment(name: &str) -> io::Result<String> {
    proc_macro::tracked::env_var(name).map_err(io::Error::other)
}

/// Resolves the shared output policy and tracks its optional build-only override.
fn output_root(anchor: &Path) -> io::Result<PathBuf> {
    let explicit = match proc_macro::tracked::env_var("NEOCOGI_NATIVE_OUTPUT_DIR") {
        Ok(value) => Some(PathBuf::from(value)),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => return Err(io::Error::other(error)),
    };
    profile::output_root(anchor, explicit)
}

/// Publishes changed contents atomically; identical files retain their mtime.
/// Existing mappings remain valid on Unix because their files are not truncated.
fn copy_file(source: &Path, destination: &Path) -> io::Result<()> {
    let bytes = fs::read(source)?;
    if fs::read(destination).ok().as_deref() != Some(bytes.as_slice()) {
        let directory = destination.parent().ok_or_else(|| io::Error::other("staged file has no directory"))?;
        fs::create_dir_all(directory)?;
        let temporary = tempfile::NamedTempFile::new_in(directory)?;
        fs::copy(source, temporary.path())?;
        temporary.persist(destination).map_err(|error| error.error)?;
    }
    // Tracking the deployed file catches removal and another cached variant
    // replacing it. It is not an embedded include_bytes! payload.
    proc_macro::tracked::path(source);
    proc_macro::tracked::path(destination);
    Ok(())
}

/// Copies a selected normal cdylib artifact and expands to its platform filename.
///
/// The argument names Cargo's CARGO_CDYLIB_FILE_* variable. Absolute source paths
/// are used only by the compiler. Runtime callers join the filename to app/lib.
#[proc_macro]
pub fn stage_library(input: TokenStream) -> TokenStream {
    let variable = syn::parse_macro_input!(input as LitStr);
    let result = (|| -> io::Result<String> {
        let source = PathBuf::from(environment(&variable.value())?);
        let filename = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io::Error::other("Cargo library filename must be UTF-8"))?;
        let root = output_root(&source)?;
        copy_file(&source, &root.join("lib").join(filename))?;
        // Windows resolves initial imports beside the executable before main.
        if filename == "nc_com.dll" {
            copy_file(&source, &root.join(filename))?;
            for import in ["nc_com.dll.lib", "libnc_com.dll.a"] {
                let input = source.with_file_name(import);
                if input.is_file() {
                    copy_file(&input, &root.join("lib-imports").join(import))?;
                }
            }
        }
        Ok(filename.to_owned())
    })();
    match result {
        Ok(filename) => quote!(#filename).into(),
        Err(error) => diagnostic(error),
    }
}

/// One authored source path and its application-relative destination.
struct Resource {
    /// File or directory relative to the consuming package's manifest.
    source: LitStr,
    /// Destination owned by the application layout, with no parent traversal.
    destination: LitStr,
}
impl Parse for Resource {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let source = input.parse()?;
        input.parse::<Token![=>]>()?;
        Ok(Self { source, destination: input.parse()? })
    }
}

/// Copies a resource tree and tracks directory membership and individual files.
fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    if source.is_dir() {
        proc_macro::tracked::path(source);
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        copy_file(source, destination)
    }
}

/// Copies literal source => relative-destination resource pairs; emits no runtime paths.
/// Existing embed-assets/browser builds omit this macro through cfg selection.
#[proc_macro]
pub fn stage_resources(input: TokenStream) -> TokenStream {
    let resources = syn::parse_macro_input!(input with Punctuated::<Resource, Token![,]>::parse_terminated);
    let result = (|| -> io::Result<()> {
        let manifest = PathBuf::from(environment("CARGO_MANIFEST_DIR")?);
        let root = output_root(Path::new(&environment("OUT_DIR")?))?;
        for resource in resources {
            let relative = PathBuf::from(resource.destination.value());
            if relative.components().any(|part| !matches!(part, Component::Normal(_) | Component::CurDir)) {
                return Err(io::Error::other("resource destination must stay inside the application directory"));
            }
            copy_tree(&manifest.join(resource.source.value()), &root.join(relative))?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => quote!(()).into(),
        Err(error) => diagnostic(error),
    }
}
