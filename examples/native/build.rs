//! Optional local artwork is staged as file slots; ordinary builds never download assets.
/// Stages present local artwork using the contracts crate's shared resource ownership policy.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(example_assets_available)");
    println!("cargo::rerun-if-changed=gltf/assets");
    let root = std::path::Path::new("gltf/assets");
    if ["DamagedHelmet.glb", "BrainStem.glb", "Cannon_Exterior.hdr"]
        .iter()
        .all(|name| root.join(name).is_file())
    {
        println!("cargo::rustc-cfg=example_assets_available");
        use neocogi_platform_contracts::build::layout::{Resource, output_root, stage_resources};
        let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        println!("cargo::rerun-if-env-changed=NEOCOGI_NATIVE_OUTPUT_DIR");
        let anchor = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let output = output_root(Some(&anchor), std::env::var_os("NEOCOGI_NATIVE_OUTPUT_DIR").map(Into::into)).unwrap();
        let names = [
            "assets/DamagedHelmet.glb",
            "assets/BrainStem.glb",
            "assets/Cannon_Exterior.hdr",
            "DamagedHelmet-LICENSE.md",
            "BrainStem-LICENSE.md",
            "LicenseRef-Poser-EULA.txt",
            "Cannon_Exterior-LICENSE.md",
            "Cannon_Exterior-CC0.txt",
        ];
        let resources = names
            .iter()
            .map(|name| Resource {
                source: manifest.join("gltf").join(name),
                destination: std::path::Path::new("assets/gltf").join(std::path::Path::new(name).file_name().unwrap()),
            })
            .collect::<Vec<_>>();
        let staged = stage_resources(&manifest, &output, &resources).expect("stage example artwork");
        for path in staged.inputs.into_iter().chain(staged.outputs) {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
}
