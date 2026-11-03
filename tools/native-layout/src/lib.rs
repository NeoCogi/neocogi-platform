//! Copies Cargo-selected libraries and exact authored resource snapshots at compile time.
//!
//! No nested Cargo graph is created. Tracked inputs/outputs invalidate cached expansion.
//! Resources exclusively own nonoverlapping destination subtrees; removed members are
//! pruned. Runtime library code contains filenames rather than absolute build paths.
#![feature(proc_macro_tracked_path, proc_macro_tracked_env)]
#![deny(missing_docs)]

use std::{io, path::PathBuf};
use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    LitStr, Token,
};
use neocogi_native_layout_core as layout;

/// Converts a build/staging failure into a focused compiler error.
fn diagnostic(error: impl std::fmt::Display) -> TokenStream {
    syn::Error::new(proc_macro2::Span::call_site(), error.to_string()).into_compile_error().into()
}

/// Reads one required compiler environment value without emitting it into runtime code.
fn environment(name: &str) -> io::Result<String> {
    proc_macro::tracked::env_var(name).map_err(io::Error::other)
}

/// Reads an optional tracked value, preserving invalid-Unicode errors.
fn optional_environment(name: &str) -> io::Result<Option<String>> {
    match proc_macro::tracked::env_var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(io::Error::other(error)),
    }
}

/// Explicit output needs no OUT_DIR; inference requires a consumer build.rs.
fn output_root() -> io::Result<PathBuf> {
    let explicit = optional_environment("NEOCOGI_NATIVE_OUTPUT_DIR")?.map(PathBuf::from);
    let anchor = if explicit.is_none() {
        optional_environment("OUT_DIR")?.map(PathBuf::from)
    } else {
        None
    };
    layout::output_root(anchor.as_deref(), explicit)
}

/// Stages a normal Cargo cdylib artifact and expands to its platform filename.
///
/// The argument names Cargo's CARGO_CDYLIB_FILE_* variable. Runtime callers join
/// the result to app/lib. An absolute NEOCOGI_NATIVE_OUTPUT_DIR works without
/// build.rs; automatic consumer-root inference requires its OUT_DIR.
#[proc_macro]
pub fn stage_library(input: TokenStream) -> TokenStream {
    let variable = syn::parse_macro_input!(input as LitStr);
    let result = (|| -> io::Result<String> {
        let source = PathBuf::from(environment(&variable.value())?);
        let root = output_root()?;
        let filename = layout::stage_library(&source, &root)?;
        proc_macro::tracked::path(&source);
        proc_macro::tracked::path(root.join("lib").join(&filename));
        Ok(filename)
    })();
    match result {
        Ok(filename) => quote!(#filename).into(),
        Err(error) => diagnostic(error),
    }
}

/// One authored input and its exclusively owned application-relative destination.
struct Resource {
    /// Input path relative to the consumer manifest, or an explicit absolute build input.
    source: LitStr,
    /// Destination slot, with parent traversal, root and reserved namespaces forbidden.
    destination: LitStr,
}
impl Parse for Resource {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let source = input.parse()?;
        input.parse::<Token![=>]>()?;
        Ok(Self { source, destination: input.parse()? })
    }
}

/// Stages exclusively owned source => relative-destination resource snapshots.
///
/// Removed members are pruned; overlapping destinations and preexisting unclaimed
/// output reject. Rebuilding the same package/destination replaces that slot. A
/// destination cannot be the output root, lib, or native-layout metadata. Inputs
/// and destination ancestry must not traverse symlinks. An absolute explicit root
/// works without build.rs; inferred output requires Cargo OUT_DIR.
#[proc_macro]
pub fn stage_resources(input: TokenStream) -> TokenStream {
    let resources = syn::parse_macro_input!(input with Punctuated::<Resource, Token![,]>::parse_terminated);
    let result = (|| -> io::Result<()> {
        let manifest = PathBuf::from(environment("CARGO_MANIFEST_DIR")?);
        let root = output_root()?;
        let resources = resources
            .into_iter()
            .map(|resource| layout::Resource {
                source: manifest.join(resource.source.value()),
                destination: resource.destination.value().into(),
            })
            .collect::<Vec<_>>();
        let staged = layout::stage_resources(&manifest, &root, &resources)?;
        for path in staged.inputs.into_iter().chain(staged.outputs) {
            proc_macro::tracked::path(path);
        }
        Ok(())
    })();
    match result {
        Ok(()) => quote!(()).into(),
        Err(error) => diagnostic(error),
    }
}
