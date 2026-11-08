//! Example-owned command-line bootstrap shared by all native binaries.
use crate::sdk::render::{abi::RenderError, platform::PlatformAbi};
use neocogi_com::ComPtr;

/// Runtime backend request separated from each application's remaining arguments.
pub struct LaunchArguments {
    /// User spelling is resolved to the loader's typed backend before any plugin/native work.
    pub backend: Option<String>,
    /// Model, rendering or triangle arguments, kept in their original order.
    pub arguments: Vec<String>,
}
impl LaunchArguments {
    /// Remove one --backend NAME option; duplicate/missing values are ordinary input errors.
    pub fn parse(args: impl Iterator<Item = String>) -> Result<Self, RenderError> {
        let mut args = args;
        let mut result = Self { backend: None, arguments: Vec::new() };
        while let Some(argument) = args.next() {
            if argument == "--backend" {
                if result.backend.is_some() {
                    return Err(RenderError::new(neocogi_com::NC_E_INVALID_ARG, "--backend may be specified only once"));
                }
                let value = args
                    .next()
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or_else(|| RenderError::new(neocogi_com::NC_E_INVALID_ARG, "--backend requires a name: glow, vulkan or wgpu"))?;
                result.backend = Some(value);
            } else {
                result.arguments.push(argument);
            }
        }
        Ok(result)
    }
}

/// Acquire one process-lived platform root using linked static code or a runtime-selected library.
/// Static requests may name only the compiled backend; dynamic requests never enable SDK features.
pub fn acquire_platform(requested: Option<&str>) -> Result<ComPtr<PlatformAbi>, RenderError> {
    #[cfg(feature = "static")]
    if let Some(requested) = requested {
        let compiled = if cfg!(feature = "vulkan") {
            "vulkan"
        } else if cfg!(feature = "wgpu") {
            "wgpu"
        } else {
            "glow"
        };
        if requested != compiled {
            return Err(RenderError::new(
                neocogi_com::NC_E_INVALID_ARG,
                format!("This static application uses {compiled}; rebuild its backend or use dynamic assembly for --backend {requested}"),
            ));
        }
    }
    // SAFETY: This final executable owns the immutable process-lived host tables.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }?;
    #[cfg(feature = "static")]
    {
        // SAFETY: The complete descriptor graph and implementation stay linked for process lifetime.
        unsafe {
            runtime.register_plugin_descriptor(crate::sdk::platform::plugin_descriptor())?;
        }
        runtime
            .acquire_plugin_root(crate::sdk::platform::plugin_descriptor().plugin_id)
            .map_err(Into::into)
    }
    #[cfg(feature = "dynamic")]
    neocogi_example_plugins::acquire(runtime, requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runtime selection may appear before or after application flags without reordering their values.
    #[test]
    fn backend_arguments_preserve_application_options_and_reject_missing_or_duplicate_values() {
        for args in [vec!["--backend", "vulkan", "--frames", "3"], vec!["--frames", "3", "--backend", "vulkan"]] {
            let parsed = LaunchArguments::parse(args.into_iter().map(str::to_owned)).unwrap();
            assert_eq!(parsed.backend.as_deref(), Some("vulkan"));
            assert_eq!(parsed.arguments, ["--frames", "3"]);
        }
        let empty = LaunchArguments::parse(std::iter::empty()).unwrap();
        assert!(empty.backend.is_none() && empty.arguments.is_empty());
        for args in [
            vec!["--backend"],
            vec!["--backend", ""],
            vec!["--backend", "--frames", "3"],
            vec!["--backend", "glow", "--backend", "vulkan"],
        ] {
            assert!(LaunchArguments::parse(args.into_iter().map(str::to_owned)).is_err());
        }
    }

    /// A runtime request cannot change the implementation linked by a static build.
    #[test]
    #[cfg(feature = "static")]
    fn static_assembly_rejects_other_backends_before_host_or_native_initialization() {
        let other = if cfg!(feature = "glow") { "vulkan" } else { "glow" };
        let error = acquire_platform(Some(other)).unwrap_err();
        assert_eq!(error.status, neocogi_com::NC_E_INVALID_ARG);
        assert!(error.message.contains("static application"));
    }
}
