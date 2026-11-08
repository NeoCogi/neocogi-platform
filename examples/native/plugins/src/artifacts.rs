//! One authoritative backend/target filename table shared by staging and runtime loading.

/// Exactly one selected platform assembly, shared by staging and runtime loading.
#[derive(Clone, Copy)]
pub enum Backend {
    /// GLES3 provider and its GLSL compiler.
    Glow,
    /// Vulkan provider and its SPIR-V compiler.
    Vulkan,
    /// WGPU provider and its WGSL compiler.
    Wgpu,
}
/// Library filename convention follows the application target, not its build-script host.
#[derive(Clone, Copy)]
pub enum System {
    /// Windows libraries have no prefix and use the DLL suffix.
    Windows,
    /// macOS libraries use the lib prefix and dylib suffix.
    Macos,
    /// Other supported native targets use the lib prefix and so suffix.
    Unix,
}
/// Features are identical for this package's build script and runtime library.
pub const fn selected_backend() -> Backend {
    if cfg!(feature = "vulkan") {
        Backend::Vulkan
    } else if cfg!(feature = "wgpu") {
        Backend::Wgpu
    } else {
        Backend::Glow
    }
}
/// Exact name of the selected platform library; its renderer owns shader compilation.
pub const fn libraries(backend: Backend, system: System) -> [&'static str; 1] {
    match (backend, system) {
        (Backend::Glow, System::Windows) => ["nc_platform_glow.dll"],
        (Backend::Glow, System::Macos) => ["libnc_platform_glow.dylib"],
        (Backend::Glow, System::Unix) => ["libnc_platform_glow.so"],
        (Backend::Vulkan, System::Windows) => ["nc_platform_vulkan.dll"],
        (Backend::Vulkan, System::Macos) => ["libnc_platform_vulkan.dylib"],
        (Backend::Vulkan, System::Unix) => ["libnc_platform_vulkan.so"],
        (Backend::Wgpu, System::Windows) => ["nc_platform_wgpu.dll"],
        (Backend::Wgpu, System::Macos) => ["libnc_platform_wgpu.dylib"],
        (Backend::Wgpu, System::Unix) => ["libnc_platform_wgpu.so"],
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Platform libraries have unambiguous target/backend names with no paths.
    #[test]
    fn platform_filenames_follow_target_conventions_and_remain_backend_specific() {
        for (backend, name) in [(Backend::Glow, "glow"), (Backend::Vulkan, "vulkan"), (Backend::Wgpu, "wgpu")] {
            for (system, prefix, suffix) in [(System::Windows, "", ".dll"), (System::Macos, "lib", ".dylib"), (System::Unix, "lib", ".so")] {
                assert_eq!(libraries(backend, system), [format!("{prefix}nc_platform_{name}{suffix}")]);
            }
        }
    }
}
