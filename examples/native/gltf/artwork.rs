//! Required native glTF deployment inputs shared by staging, downloads and qualification.

/// Pinned DamagedHelmet download filename.
pub const DAMAGED_HELMET: &str = "DamagedHelmet.glb";
/// Pinned BrainStem download filename.
pub const BRAINSTEM: &str = "BrainStem.glb";
/// Pinned environment download filename.
pub const CANNON_EXTERIOR: &str = "Cannon_Exterior.hdr";

/// Downloaded files, relative to the authored `assets/` directory.
pub const DOWNLOADS: [&str; 3] = [DAMAGED_HELMET, BRAINSTEM, CANNON_EXTERIOR];
/// License files checked in alongside the authored `assets/` directory.
pub const LICENSES: [&str; 5] = [
    "DamagedHelmet-LICENSE.md",
    "BrainStem-LICENSE.md",
    "LicenseRef-Poser-EULA.txt",
    "Cannon_Exterior-LICENSE.md",
    "Cannon_Exterior-CC0.txt",
];
/// Deployment slot containing all three downloaded files and their licenses.
pub const STAGED_DIR: &str = "assets/gltf";
