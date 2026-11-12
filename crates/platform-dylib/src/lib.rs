//! Final dynamic link root for neocogi-platform::platform; reusable code has no loader export.
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
    unsafe { neocogi_com::return_plugin_descriptor(out, neocogi_platform::platform::plugin_descriptor()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The discovery ABI borrows the exact descriptor and rejects a null output
    /// without invoking a factory, initializing SDL or installing a runtime.
    #[test]
    fn discovery_preserves_descriptor_identity_and_rejects_null_output() {
        // SAFETY: Null is explicitly accepted by the discovery ABI and rejected.
        assert_eq!(unsafe { nc_get_plugin_descriptor_v1(core::ptr::null_mut()) }, neocogi_com::NC_E_POINTER);
        let mut descriptor = core::ptr::null();
        // SAFETY: The output is one aligned writable descriptor pointer slot.
        assert_eq!(unsafe { nc_get_plugin_descriptor_v1(&mut descriptor) }, neocogi_com::NC_OK);
        assert!(core::ptr::eq(descriptor, neocogi_platform::platform::plugin_descriptor()));
        // SAFETY: Discovery lends an immutable static descriptor graph.
        assert_eq!(unsafe { neocogi_com::validate_plugin_descriptor(descriptor) }, neocogi_com::NC_OK);
    }
}
