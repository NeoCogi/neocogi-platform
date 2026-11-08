//! Explicit, pinned sample downloads; build scripts never perform network I/O.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{fs, io, path::Path, time::Duration};

/// One upstream object with an immutable revision URL and SHA-256 digest.
struct Asset<'a> {
    /// Basename beneath the requested destination directory.
    name: &'static str,
    /// Immutable upstream source, independent of mutable branch tips.
    url: &'static str,
    /// Lowercase hex SHA-256 of the exact authored bytes.
    digest: &'a str,
}

/// Model and environment revisions are independently pinned; licenses remain checked in beside assets.
const ASSETS: &[Asset<'static>] = &[
    Asset {
        name: "DamagedHelmet.glb",
        url: "https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/8f492390ddc7a2ae221a1369ef560589cf8c70fb/Models/DamagedHelmet/glTF-Binary/DamagedHelmet.glb",
        digest: "a1e3b04de97b11de564ce6e53b95f02954a297f0008183ac63a4f5974f6b32d8",
    },
    Asset {
        name: "BrainStem.glb",
        url: "https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/8f492390ddc7a2ae221a1369ef560589cf8c70fb/Models/BrainStem/glTF-Binary/BrainStem.glb",
        digest: "ddda3a53b5c9771f992463034a430340ba2bac571e5869c723f7cba4976e9174",
    },
    Asset {
        name: "Cannon_Exterior.hdr",
        url: "https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Environments/3a030b54c2e4f0f4d469eec2f98f47398c9a2a30/Cannon_Exterior.hdr",
        digest: "7d65926ed2fa41c121738af2addf1fd5f1ae42d12ff77255e2679777f06c984b",
    },
];

/// Hash a stream without retaining another full model/HDR allocation.
fn digest(reader: impl io::Read) -> Result<String> {
    let mut hash = Sha256::new();
    io::copy(&mut io::BufReader::new(reader), &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Verify caching first, then stage in the destination filesystem and atomically replace on success.
fn verified_download(path: &Path, asset: &Asset<'_>, download: impl FnOnce(&mut fs::File) -> Result<()>) -> Result<()> {
    match fs::File::open(path) {
        Ok(file) => {
            if digest(file)? == asset.digest {
                println!("Verified {}", path.display());
                return Ok(());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    }
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().context("asset has no parent directory")?)?;
    download(temporary.as_file_mut()).with_context(|| format!("download {}", asset.url))?;
    ensure!(
        digest(fs::File::open(temporary.path())?)? == asset.digest,
        "SHA-256 mismatch for {}; existing file preserved",
        path.display()
    );
    temporary.persist(path).with_context(|| format!("install {}", path.display()))?;
    println!("Downloaded {}", path.display());
    Ok(())
}

/// Fetch only missing/corrupt assets using Rust HTTPS, with a bounded network deadline.
pub(crate) fn fetch(destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into();
    for asset in ASSETS {
        verified_download(&destination.join(asset.name), asset, |file| {
            io::copy(&mut agent.get(asset.url).call()?.body_mut().as_reader(), file)?;
            Ok(())
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Corruption and transport failure cannot replace a file or leave staging debris.
    #[test]
    fn verified_cache_and_atomic_install() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Model.glb");
        let expected = digest(&b"valid"[..]).unwrap();
        let asset = Asset {
            name: "Model.glb",
            url: "https://example.test/model",
            digest: &expected,
        };
        fs::write(&path, b"old").unwrap();
        let result = verified_download(&path, &asset, |file| {
            file.write_all(b"corrupt")?;
            Ok(())
        });
        assert!(result.unwrap_err().to_string().contains("SHA-256 mismatch"));
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert!(
            verified_download(&path, &asset, |file| {
                file.write_all(b"partial")?;
                anyhow::bail!("transport failure")
            })
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        verified_download(&path, &asset, |file| {
            file.write_all(b"valid")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"valid");
        verified_download(&path, &asset, |_| panic!("verified cache must not access the network")).unwrap();
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    /// The environment keeps its own upstream revision and integrity pin.
    #[test]
    fn pins_models_and_environment_separately() {
        assert_eq!(ASSETS.len(), 3);
        assert!(ASSETS[0].url.contains("8f492390ddc7a2ae221a1369ef560589cf8c70fb"));
        assert!(ASSETS[1].url.contains("8f492390ddc7a2ae221a1369ef560589cf8c70fb"));
        assert!(ASSETS[2].url.contains("glTF-Sample-Environments/3a030b54c2e4f0f4d469eec2f98f47398c9a2a30"));
        assert_eq!(ASSETS[2].name, "Cannon_Exterior.hdr");
        for asset in ASSETS {
            assert_eq!(asset.digest.len(), 64);
        }
    }
}
