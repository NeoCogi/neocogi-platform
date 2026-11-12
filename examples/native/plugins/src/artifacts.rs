//! Target library naming and build-selected backend identities for staging and loading.

/// Known build-selected platform identities; an optional runtime request verifies the deployed one.
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
    /// Every recognized native platform descriptor.
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
    /// Expected descriptor identity; the deployed library must match an explicit request.
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
/// One normal Cargo cdylib filename on each native target, independent of its compiled backend.
pub const fn library_filename(system: System) -> &'static str {
    match system {
        System::Windows => "nc_platform.dll",
        System::Macos => "libnc_platform.dylib",
        System::Unix => "libnc_platform.so",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Native targets use exactly the public library target's filename, with no backend suffix.
    #[test]
    fn platform_filenames_follow_target_conventions() {
        for (system, prefix, suffix) in [(System::Windows, "", ".dll"), (System::Macos, "lib", ".dylib"), (System::Unix, "lib", ".so")] {
            assert_eq!(library_filename(system), format!("{prefix}nc_platform{suffix}"));
        }
    }
}
