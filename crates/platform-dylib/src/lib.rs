//! Final dynamic link root for neocogi-platform::platform; reusable code has no loader export.
//! The descriptor, acquisition policy and COM objects remain owned by the implementation.
#![deny(missing_docs)]

include!("platform_entry.rs");
