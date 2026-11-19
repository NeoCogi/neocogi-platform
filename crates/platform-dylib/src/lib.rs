//! Final dynamic link root for neocogi-platform::platform; reusable code has no loader export.
//! The descriptor, acquisition policy and COM objects remain owned by the implementation.
#![deny(missing_docs)]

/// Returns the implementation's immutable descriptor through the discovery ABI.
///
/// # Safety
/// `out` must be null or aligned and writable for one descriptor pointer. The caller
/// must retain this library while using any reachable metadata or implementation code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nc_get_plugin_descriptor_v3(out: *mut *const neocogi_com::NcPluginDescriptor) -> neocogi_com::NcStatus {
    // SAFETY: the caller supplies the output slot; the complete static descriptor graph
    // and its acquisition functions remain mapped as part of this final library.
    unsafe { neocogi_com::return_plugin_descriptor(out, neocogi_platform::platform::plugin_descriptor()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neocogi_com::{ComPtr, IUnknown, NC_COM_CORE_ABI_VERSION, NC_COM_RUNTIME_ABI_VERSION, NC_E_ABI_VERSION, NcHostApiV3};

    /// The discovery ABI borrows the exact descriptor and rejects a null output
    /// without invoking a factory, initializing SDL or installing a runtime.
    #[test]
    fn discovery_preserves_descriptor_identity_and_rejects_null_output() {
        // Keep the actual exported spelling synchronized with the SDK loader ABI.
        assert_eq!(neocogi_com::NC_GET_PLUGIN_DESCRIPTOR_SYMBOL, stringify!(nc_get_plugin_descriptor_v3));
        // SAFETY: Null is explicitly accepted by the discovery ABI and rejected.
        assert_eq!(unsafe { nc_get_plugin_descriptor_v3(core::ptr::null_mut()) }, neocogi_com::NC_E_POINTER);
        let mut descriptor = core::ptr::null();
        // SAFETY: The output is one aligned writable descriptor pointer slot.
        assert_eq!(unsafe { nc_get_plugin_descriptor_v3(&mut descriptor) }, neocogi_com::NC_OK);
        assert!(core::ptr::eq(descriptor, neocogi_platform::platform::plugin_descriptor()));
        // SAFETY: Discovery lends an immutable static descriptor graph.
        assert_eq!(unsafe { neocogi_com::validate_plugin_descriptor(descriptor) }, neocogi_com::NC_OK);
    }

    /// The generated provider bootstrap clears the factory output before rejecting
    /// the old runtime family or a host with current family and an older core.
    #[test]
    fn acquisition_clears_output_for_incompatible_host_versions() {
        use neocogi_platform::render::compiler::Factory;

        let acquire = neocogi_platform::platform::plugin_descriptor().acquire.unwrap();
        let seed: ComPtr<IUnknown> = Factory::from_value(Factory).query().unwrap();
        for (runtime_abi_version, core_abi_version) in [(2, 19), (2, NC_COM_CORE_ABI_VERSION), (NC_COM_RUNTIME_ABI_VERSION, 19)] {
            let host = NcHostApiV3 {
                size: core::mem::size_of::<NcHostApiV3>(),
                runtime_abi_version,
                core_abi_version,
                runtime: core::ptr::null(),
                debug: core::ptr::null(),
                persistence: core::ptr::null(),
            };
            let mut output = seed.as_raw();
            // SAFETY: The aligned host record supplies an initialized version prefix.
            // Its versions require rejection before reading service pointers. The
            // distinct writable output slot starts with a borrowed pair held by seed.
            assert_eq!(unsafe { acquire(&host, &mut output) }, NC_E_ABI_VERSION);
            assert!(output.is_null());
        }
    }
}
