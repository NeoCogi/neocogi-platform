//! Numerical and raw-storage regressions; ordinary tests never need assets or a GPU.
use super::*;

/// Small deterministic preparation retains the production math and every atlas region.
const SMALL: Quality = Quality {
    specular_width: 8,
    diffuse_width: 4,
    lut_size: 8,
    samples: 128,
};

/// An HDR constant signal reveals normalization errors in either lighting convolution.
fn constant() -> Panorama {
    Panorama {
        width: 8,
        height: 4,
        pixels: vec![[2., 0.5, 4.]; 32],
    }
}

/// RGBE preserves highlights above one and black without any display transfer function.
#[test]
fn radiance_encoding_keeps_hdr_range() {
    for color in [[0.; 3], [0.001, 0.5, 2.], [65504., 16., 1.], [1e-20; 3]] {
        let decoded = decode_rgbe(encode_rgbe(color));
        let peak = color.into_iter().fold(0., f32::max);
        for channel in 0..3 {
            assert!((decoded[channel] - color[channel]).abs() <= peak / 128. + 1e-30);
        }
    }
    assert_eq!(encode_rgbe([0.; 3]), [0; 4]);
}

/// Both source interpolation and filtered radiance preserve a constant environment's energy.
#[test]
fn constant_environment_is_preserved_at_all_roughnesses() {
    let panorama = constant();
    let atlas = Environment::prepare(&panorama, SMALL).unwrap();
    for index in 0..=DIFFUSE_REGION {
        let region = atlas.regions[index];
        for y in 0..region.height {
            for x in 0..region.width {
                let offset = ((region.y + y) * atlas.width + x) * 4;
                let decoded = decode_rgbe(atlas.pixels[offset as usize..offset as usize + 4].try_into().unwrap());
                for (actual, expected) in decoded.into_iter().zip([2., 0.5, 4.]) {
                    assert!((actual - expected).abs() < 0.04, "region {index}: {decoded:?}");
                }
            }
        }
    }
    assert_eq!(atlas.uniforms(1., 1.).len(), UNIFORM_BYTES as usize);
}

/// Longitude repeats while polar rows clamp; standard axes agree with the shader mapping.
#[test]
fn equirectangular_orientation_and_seam_are_consistent() {
    let panorama = Panorama {
        width: 4,
        height: 2,
        pixels: vec![
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [1., 1., 0.],
            [0.5, 0., 0.],
            [0., 0.5, 0.],
            [0., 0., 0.5],
            [0.5, 0.5, 0.],
        ],
    };
    for (u, v) in [(0.125, 0.25), (0.375, 0.25), (0.625, 0.75), (0.875, 0.75)] {
        let actual = panorama.sample(direction(u, v));
        let expected = panorama.pixels[((v * 2.) as usize) * 4 + (u * 4.) as usize];
        for (a, b) in actual.into_iter().zip(expected) {
            assert!((a - b).abs() < 1e-5);
        }
    }
    let a = panorama.sample(direction(0.000001, 0.5));
    let b = panorama.sample(direction(0.999999, 0.5));
    assert!(a.iter().zip(b).all(|(a, b)| (*a - b).abs() < 0.0001));
    assert!(panorama.sample([0., 1., 0.]).iter().all(|v| v.is_finite()));
    assert!(panorama.sample([0., -1., 0.]).iter().all(|v| v.is_finite()));
}

/// GGX filtering broadens a directional signal; all prefiltered samples remain bounded radiance.
#[test]
fn roughness_filters_the_environment_and_brdf_stays_finite() {
    let mut source = constant();
    for (index, pixel) in source.pixels.iter_mut().enumerate() {
        let x = index % 8;
        *pixel = if (3..=4).contains(&x) { [8.; 3] } else { [0.; 3] };
    }
    let atlas = Environment::prepare(&source, SMALL).unwrap();
    let region = atlas.regions[SPECULAR_LEVELS - 1];
    let mut has_blur = false;
    for y in 0..region.height {
        for x in 0..region.width {
            let offset = (((region.y + y) * atlas.width + x) * 4) as usize;
            let color = decode_rgbe(atlas.pixels[offset..offset + 4].try_into().unwrap());
            assert!(color.iter().all(|v| v.is_finite() && *v >= 0. && *v <= 8.));
            has_blur |= color[0] > 0.1 && color[0] < 7.9;
        }
    }
    assert!(has_blur);
    for nv in [0.001, 0.1, 0.5, 0.999] {
        for roughness in [0.001, 0.2, 0.8, 1.] {
            let response = integrate_brdf(nv, roughness, 1024);
            assert!(
                response.iter().all(|v| v.is_finite() && *v >= 0. && *v <= 1.01),
                "{nv} {roughness}: {response:?}"
            );
            assert!(response[0] + response[1] <= 1.02);
        }
    }
}

/// Cache keys include source/quality; corruption, stale sources and invalid extents cannot reuse.
#[test]
fn cache_validates_payload_and_exact_raw_storage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("environment.ibl");
    let atlas = Environment::prepare(&constant(), SMALL).unwrap();
    let key = cache_key(b"source", SMALL);
    atlas.write_cache(&path, key);
    let restored = Environment::read_cache(&path, key).unwrap();
    assert_eq!(restored.pixels, atlas.pixels);
    assert_eq!(restored.regions, atlas.regions);
    assert!(Environment::read_cache(&path, cache_key(b"different", SMALL)).is_none());
    assert_ne!(key, cache_key(b"source", Quality { samples: 64, ..SMALL }));
    let original = fs::read(&path).unwrap();
    for count in [0, 39, HEADER_BYTES - 1, original.len() - 1] {
        fs::write(&path, &original[..count]).unwrap();
        assert!(Environment::read_cache(&path, key).is_none());
    }
    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(&path, corrupt).unwrap();
    assert!(Environment::read_cache(&path, key).is_none());
    let mut corrupt = original;
    corrupt[80 + 8..80 + 12].copy_from_slice(&u32::MAX.to_le_bytes());
    let checksum = Sha256::digest(&corrupt[72..]);
    corrupt[40..72].copy_from_slice(&checksum);
    fs::write(&path, corrupt).unwrap();
    assert!(Environment::read_cache(&path, key).is_none());
}

/// The real HDR decoder retains values above one; malformed headers and missing files diagnose.
#[test]
fn decodes_hdr_as_floating_point_and_rejects_bad_sources() {
    let mut bytes = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut bytes)
        .encode(&[image::Rgb([4., 0.5, 2.]); 8], 4, 2)
        .unwrap();
    let source = Panorama::decode(&bytes).unwrap();
    assert_eq!(source.pixels[0], [4., 0.5, 2.]);
    assert!(Panorama::decode(b"not an HDR").is_err());
    let huge = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2049 +X 4096\n";
    assert!(Panorama::decode(huge).is_err());
    assert!(
        Environment::load(Path::new("/does-not-exist/environment.hdr"))
            .err()
            .unwrap()
            .contains("cargo tools fetch-gltf-examples")
    );
}

/// Opt-in real-source preparation additionally proves the persisted atlas round-trips.
#[test]
#[ignore = "requires cargo tools fetch-gltf-examples"]
fn cannon_exterior_prepares_and_reuses_cache() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("gltf/assets/Cannon_Exterior.hdr");
    let atlas = Environment::load(&path).unwrap();
    assert_eq!(atlas.regions[0].width, 1024);
    assert_eq!(atlas.regions[0].height, 512);
    assert!(atlas.pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 128));
    let cached = Environment::load(&path).unwrap();
    assert_eq!(atlas.pixels, cached.pixels);
}
