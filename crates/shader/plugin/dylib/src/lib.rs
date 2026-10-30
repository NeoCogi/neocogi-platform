//! Final dynamic link root for neocogi-shader-compiler; reusable code has no loader export.
//! The descriptor, acquisition policy and COM objects remain owned by the implementation.
#![deny(missing_docs)]

/// Returns the implementation's immutable descriptor through the discovery ABI.
///
/// # Safety
/// `out` must be null or aligned and writable for one descriptor pointer. The caller
/// must retain this library while using any reachable metadata or implementation code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nc_get_plugin_descriptor_v1(out: *mut *const neocogi_com::NcPluginDescriptor) -> neocogi_com::NcStatus {
    // SAFETY: the caller supplies the output slot; the complete static descriptor graph
    // and its acquisition functions remain mapped as part of this final library.
    unsafe { neocogi_com::return_plugin_descriptor(out, provider::plugin_descriptor()) }
}
