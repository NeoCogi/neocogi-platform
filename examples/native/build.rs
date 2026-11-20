//! Optional local artwork is staged as file slots; ordinary builds never download assets.
#[path = "gltf/artwork.rs"]
mod artwork;
use artwork::{DOWNLOADS, LICENSES, STAGED_DIR};

/// Stages present local artwork using the contracts crate's shared resource ownership policy.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(example_assets_available)");
    println!("cargo::rerun-if-changed=gltf/assets");
    println!("cargo::rerun-if-env-changed=NEOCOGI_CARGO_PROFILE_DIR");
    println!("cargo::rerun-if-env-changed=NEOCOGI_NATIVE_OUTPUT_DIR");
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let gltf = manifest.join("gltf");
    let downloads = gltf.join("assets");
    if DOWNLOADS.iter().all(|name| downloads.join(name).is_file()) {
        use neocogi_platform_contracts::build::layout::{Resource, cargo_layout, stage_resources};
        let anchor = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let layout = cargo_layout(
            &anchor,
            std::env::var_os("NEOCOGI_CARGO_PROFILE_DIR").map(Into::into),
            std::env::var_os("NEOCOGI_NATIVE_OUTPUT_DIR").map(Into::into),
        )
        .expect("resolve example artwork layout");
        let Some(output) = layout.destination else {
            println!("cargo::warning=Artwork staging needs an absolute NEOCOGI_CARGO_PROFILE_DIR or NEOCOGI_NATIVE_OUTPUT_DIR");
            return;
        };
        println!("cargo::rustc-cfg=example_assets_available");
        let resources = DOWNLOADS
            .into_iter()
            .map(|name| (name, downloads.join(name)))
            .chain(LICENSES.into_iter().map(|name| (name, gltf.join(name))))
            .map(|(name, source)| Resource {
                source,
                destination: std::path::Path::new(STAGED_DIR).join(name),
            })
            .collect::<Vec<_>>();
        let staged = stage_resources(&manifest, &output, &resources).expect("stage example artwork");
        for path in staged.inputs.into_iter().chain(staged.outputs) {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
}
