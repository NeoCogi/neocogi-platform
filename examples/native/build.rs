//! Optional local artwork is staged as file slots; ordinary builds never download assets.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(example_assets_available)");
    println!("cargo::rerun-if-changed=gltf/assets");
    let root = std::path::Path::new("gltf/assets");
    if ["DamagedHelmet.glb", "BrainStem.glb", "Cannon_Exterior.hdr"]
        .iter()
        .all(|name| root.join(name).is_file())
    {
        println!("cargo::rustc-cfg=example_assets_available");
    }
}
