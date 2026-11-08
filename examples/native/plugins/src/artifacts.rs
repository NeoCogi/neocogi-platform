//! One authoritative backend/target filename table shared by staging and runtime loading.

/// Runtime platform choices, shared by staging and executable-relative discovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    /// GLES3 provider and its GLSL compiler.
    Glow,
    /// Vulkan provider and its SPIR-V compiler.
    Vulkan,
    /// WGPU provider and its WGSL compiler.
    Wgpu,
}
/// Library filename convention follows the application target, not its build-script host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum System {
    /// Windows libraries have no prefix and use the DLL suffix.
    Windows,
    /// macOS libraries use the lib prefix and dylib suffix.
    Macos,
    /// Other supported native targets use the lib prefix and so suffix.
    Unix,
}
impl Backend {
    /// Every supported native library is eligible for staging and runtime discovery.
    pub const ALL: [Self; 3] = [Self::Glow, Self::Vulkan, Self::Wgpu];
    /// Exact public CLI value, also used by the platform build command.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Glow => "glow",
            Self::Vulkan => "vulkan",
            Self::Wgpu => "wgpu",
        }
    }
    /// Parse user configuration without registering plugins or touching a native subsystem.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|backend| backend.name() == name)
    }
    /// Expected descriptor identity; a mislabeled file cannot silently select another backend.
    pub const fn plugin_name(self) -> &'static str {
        match self {
            Self::Glow => "neocogi.platform.glow",
            Self::Vulkan => "neocogi.platform.vulkan",
            Self::Wgpu => "neocogi.platform.wgpu",
        }
    }
}
impl System {
    /// Uses the application target's OS, which may differ from the build-script host.
    pub fn from_os(os: &str) -> Self {
        match os {
            "windows" => Self::Windows,
            "macos" => Self::Macos,
            _ => Self::Unix,
        }
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
