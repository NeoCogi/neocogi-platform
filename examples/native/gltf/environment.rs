//! Example-owned HDR preprocessing, independent of native graphics and scene import.
//!
//! A source panorama, cosine-weighted diffuse convolution, GGX roughness levels and
//! split-sum BRDF integral share a single 2D atlas. Radiance uses RGBE8, decoded before
//! bilinear interpolation in the shader; HDR highlights are never clipped into LDR.
//! A checked, versioned cache is keyed by the complete HDR bytes and bake parameters.

use image::ImageDecoder;
use rs_math3d::{Mat3f, Vec3f, Vector, basis_from_unit_rh};
use sha2::{Digest, Sha256};
use std::{
    f32::consts::{PI, TAU},
    fs,
    io::Cursor,
    path::Path,
};

/// Lighting layout is shared with the exact shader sources qualified by the native corpus.
pub use neocogi_platform_examples::shaders::{SPECULAR_LEVELS, DIFFUSE_REGION, BRDF_REGION, REGION_COUNT, UNIFORM_BYTES};

/// The renderer's portable RGBA8 image cap; cache validation uses this before allocation.
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Changes whenever sampling, encoding, layout or cache interpretation changes.
const MAGIC: &[u8; 8] = b"NCIBL002";
/// Header is magic, source key, payload checksum, dimensions and explicit integer rectangles.
const HEADER_BYTES: usize = 8 + 32 + 32 + 8 + REGION_COUNT * 16;

/// Shared three-component storage for scene-linear radiance and geometric integration vectors.
type Color = Vec3f;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Pixel rectangle in the single atlas, with top-left row ordering.
pub struct Region {
    /// First pixel column.
    pub x: u32,
    /// First pixel row.
    pub y: u32,
    /// Positive pixel width.
    pub width: u32,
    /// Positive pixel height.
    pub height: u32,
}

/// Complete initialized lighting data; GPU upload requires no format or backend extension.
pub struct Environment {
    /// Positive atlas width in pixels.
    pub width: u32,
    /// Positive atlas height in pixels.
    pub height: u32,
    /// Exactly width * height * 4 bytes, RGBE except the linear BRDF region.
    pub pixels: Vec<u8>,
    /// Source, GGX, diffuse and BRDF rectangles in the order defined above.
    pub regions: [Region; REGION_COUNT],
}

#[derive(Clone, Copy)]
/// Finite preprocessing cost. Tests use smaller dimensions with the same math and cache format.
struct Quality {
    /// GGX equirectangular width; height is half this value.
    specular_width: u32,
    /// Cosine-weighted diffuse equirectangular width; height is half this value.
    diffuse_width: u32,
    /// Square GGX integration lookup resolution.
    lut_size: u32,
    /// Deterministic Hammersley samples per convolution/integration output texel.
    samples: u32,
}
impl Quality {
    /// Keeps first-run preprocessing bounded; later starts reuse the complete checked atlas.
    const DEFAULT: Self = Self {
        specular_width: 256,
        diffuse_width: 32,
        lut_size: 64,
        samples: 128,
    };
}

/// Validated, decoded top-left-origin equirectangular panorama with positive finite HDR values.
struct Panorama {
    /// Positive source width.
    width: u32,
    /// Positive source height.
    height: u32,
    /// Exactly width * height linear RGB samples, each in [0, 65504].
    pixels: Vec<[f32; 3]>,
}

impl Environment {
    /// Loads the HDR source, reuses only a matching valid cache, otherwise prepares lighting.
    /// Cache persistence is best-effort: a read-only asset directory still permits rendering.
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|error| format!("{}: {error}; run cargo tools fetch-gltf-examples", path.display()))?;
        let key = cache_key(&bytes, Quality::DEFAULT);
        let cache_path = path.with_extension("ibl");
        if let Some(environment) = Self::read_cache(&cache_path, key) {
            return Ok(environment);
        }
        eprintln!("Preparing HDR environment lighting (cached after the first run)...");
        let environment = Self::prepare(&Panorama::decode(&bytes)?, Quality::DEFAULT)?;
        environment.write_cache(&cache_path, key);
        Ok(environment)
    }

    /// Serializes only explicit std140 vec4 scalars; Rust struct padding never reaches the GPU.
    pub fn uniforms(&self, exposure: f32, intensity: f32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(UNIFORM_BYTES as usize);
        for region in self.regions {
            for value in [region.x, region.y, region.width, region.height] {
                bytes.extend((value as f32).to_le_bytes());
            }
        }
        for value in [exposure, intensity, 0., 0.] {
            bytes.extend(value.to_le_bytes());
        }
        bytes
    }

    /// Deterministically computes all convolutions before publishing a complete atlas.
    fn prepare(source: &Panorama, quality: Quality) -> Result<Self, String> {
        let width = source.width.max(quality.specular_width).max(quality.diffuse_width).max(quality.lut_size);
        let mut next_y = 0;
        let regions = std::array::from_fn(|index| {
            let (width, height) = match index {
                0 => (source.width, source.height),
                DIFFUSE_REGION => (quality.diffuse_width, quality.diffuse_width / 2),
                BRDF_REGION => (quality.lut_size, quality.lut_size),
                _ => (quality.specular_width, quality.specular_width / 2),
            };
            let region = Region { x: 0, y: next_y, width, height };
            next_y += height;
            region
        });
        let mut atlas = Self {
            width,
            height: next_y,
            pixels: vec![0; image_bytes(width, next_y).ok_or("environment atlas exceeds the portable image budget")?],
            regions,
        };
        atlas.fill(0, |x, y| encode_rgbe(Color::from_array(source.pixels[(y * source.width + x) as usize])));
        for index in 1..SPECULAR_LEVELS {
            let roughness = index as f32 / (SPECULAR_LEVELS - 1) as f32;
            let half_vectors: Vec<_> = (0..quality.samples).map(|i| ggx_half(hammersley(i, quality.samples), roughness)).collect();
            let region = atlas.regions[index];
            atlas.fill(index, |x, y| {
                let n = direction((x as f32 + 0.5) / region.width as f32, (y as f32 + 0.5) / region.height as f32);
                let basis = tangent_basis(n);
                let mut sum = Color::zero();
                let mut weight = 0.;
                for &half in &half_vectors {
                    let half = basis * half;
                    let light = half * (2. * Color::dot(&n, &half)) - n;
                    let nl = Color::dot(&n, &light).max(0.);
                    if nl > 0. {
                        sum = sum + source.sample(light) * nl;
                        weight += nl;
                    }
                }
                encode_rgbe(sum * (1. / weight.max(1e-8)))
            });
        }
        let diffuse = atlas.regions[DIFFUSE_REGION];
        let cosine_samples: Vec<_> = (0..quality.samples)
            .map(|i| {
                let [u, v] = hammersley(i, quality.samples);
                let phi = TAU * v;
                Color::new(u.sqrt() * phi.cos(), u.sqrt() * phi.sin(), (1. - u).sqrt())
            })
            .collect();
        atlas.fill(DIFFUSE_REGION, |x, y| {
            let n = direction((x as f32 + 0.5) / diffuse.width as f32, (y as f32 + 0.5) / diffuse.height as f32);
            let basis = tangent_basis(n);
            let mut sum = Color::zero();
            for &sample in &cosine_samples {
                sum = sum + source.sample(basis * sample) * (1. / quality.samples as f32);
            }
            encode_rgbe(sum)
        });
        atlas.fill(BRDF_REGION, |x, y| {
            let nv = (x as f32 + 0.5) / quality.lut_size as f32;
            let roughness = (y as f32 + 0.5) / quality.lut_size as f32;
            let [scale, bias] = integrate_brdf(nv, roughness, quality.samples);
            [(scale.clamp(0., 1.) * 255.).round() as u8, (bias.clamp(0., 1.) * 255.).round() as u8, 0, 255]
        });
        Ok(atlas)
    }

    /// Writes one disjoint rectangle; all padding remains initialized zero.
    fn fill(&mut self, index: usize, pixel: impl Fn(u32, u32) -> [u8; 4]) {
        let region = self.regions[index];
        for y in 0..region.height {
            for x in 0..region.width {
                let offset = (((region.y + y) * self.width + region.x + x) * 4) as usize;
                self.pixels[offset..offset + 4].copy_from_slice(&pixel(x, y));
            }
        }
    }

    /// Reads bounded exact storage and validates every shader-visible rectangle before reuse.
    fn read_cache(path: &Path, key: [u8; 32]) -> Option<Self> {
        let size = fs::metadata(path).ok()?.len();
        if size < HEADER_BYTES as u64 || size > (MAX_BYTES + HEADER_BYTES) as u64 {
            return None;
        }
        let bytes = fs::read(path).ok()?;
        if bytes.get(..8)? != MAGIC || bytes.get(8..40)? != key {
            return None;
        }
        if Sha256::digest(bytes.get(72..)?).as_slice() != bytes.get(40..72)? {
            return None;
        }
        let integer = |offset: usize| Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?));
        let width = integer(72)?;
        let height = integer(76)?;
        if bytes.len() != HEADER_BYTES.checked_add(image_bytes(width, height)?)? {
            return None;
        }
        let mut regions = [Region { x: 0, y: 0, width: 0, height: 0 }; REGION_COUNT];
        for (index, region) in regions.iter_mut().enumerate() {
            let offset = 80 + index * 16;
            *region = Region {
                x: integer(offset)?,
                y: integer(offset + 4)?,
                width: integer(offset + 8)?,
                height: integer(offset + 12)?,
            };
            if region.width == 0 || region.height == 0 || region.x.checked_add(region.width)? > width || region.y.checked_add(region.height)? > height {
                return None;
            }
        }
        Some(Self {
            width,
            height,
            regions,
            pixels: bytes[HEADER_BYTES..].to_vec(),
        })
    }

    /// Publishes a cache by atomic rename after complete writes; failure leaves rendering usable.
    fn write_cache(&self, path: &Path, key: [u8; 32]) {
        let mut bytes = Vec::with_capacity(HEADER_BYTES + self.pixels.len());
        bytes.extend(MAGIC);
        bytes.extend(key);
        bytes.extend([0_u8; 32]);
        bytes.extend(self.width.to_le_bytes());
        bytes.extend(self.height.to_le_bytes());
        for region in self.regions {
            for value in [region.x, region.y, region.width, region.height] {
                bytes.extend(value.to_le_bytes());
            }
        }
        bytes.extend(&self.pixels);
        let checksum = Sha256::digest(&bytes[72..]);
        bytes[40..72].copy_from_slice(&checksum);
        let temporary = path.with_extension(format!("ibl.{}.tmp", std::process::id()));
        if fs::write(&temporary, bytes).is_ok() {
            let _ = fs::rename(&temporary, path);
        }
        let _ = fs::remove_file(temporary);
    }
}

impl Panorama {
    /// Decodes actual floating-point Radiance samples, with dimensions checked before allocation.
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let decoder = image::codecs::hdr::HdrDecoder::new(Cursor::new(bytes)).map_err(|e| format!("HDR header: {e}"))?;
        let (width, height) = decoder.dimensions();
        if width == 0 || height == 0 || width > 4096 || height > 2048 {
            return Err("HDR source must be at most 4096 x 2048 pixels".into());
        }
        let image = image::DynamicImage::from_decoder(decoder)
            .map_err(|e| format!("HDR pixels: {e}"))?
            .into_rgb32f();
        let pixels: Vec<_> = image.pixels().map(|p| p.0).collect();
        if pixels.iter().flatten().any(|c| !c.is_finite() || *c < 0. || *c > 65504.) {
            return Err("HDR radiance must be finite and in [0,65504]".into());
        }
        Ok(Self { width, height, pixels })
    }

    /// Bilinear sampling in linear radiance, wrapping longitude and clamping polar rows.
    fn sample(&self, direction: Color) -> Color {
        let u = 0.5 + direction.z.atan2(direction.x) / TAU;
        let v = direction.y.clamp(-1., 1.).acos() / PI;
        let x = u * self.width as f32 - 0.5;
        let y = v * self.height as f32 - 0.5;
        let ix = x.floor() as i32;
        let iy = y.floor() as i32;
        let fx = x - x.floor();
        let fy = y - y.floor();
        let mut result = Color::zero();
        for dy in 0..2 {
            for dx in 0..2 {
                let px = (ix + dx).rem_euclid(self.width as i32) as u32;
                let py = (iy + dy).clamp(0, self.height as i32 - 1) as u32;
                let weight = if dx == 0 { 1. - fx } else { fx } * if dy == 0 { 1. - fy } else { fy };
                result = result + Color::from_array(self.pixels[(py * self.width + px) as usize]) * weight;
            }
        }
        result
    }
}

/// Binds cache reuse to the source, implementation version and all quality parameters.
fn cache_key(bytes: &[u8], quality: Quality) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(MAGIC);
    hash.update(bytes);
    for value in [quality.specular_width, quality.diffuse_width, quality.lut_size, quality.samples] {
        hash.update(value.to_le_bytes());
    }
    hash.finalize().into()
}

/// Checked positive RGBA8 image size, matching the portable renderer's byte budget.
fn image_bytes(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 {
        return None;
    }
    (width as usize).checked_mul(height as usize)?.checked_mul(4).filter(|size| *size <= MAX_BYTES)
}

/// Encodes finite nonnegative radiance with a shared exponent; zero has a unique all-zero representation.
fn encode_rgbe(color: Color) -> [u8; 4] {
    let peak = color.max_abs_component();
    if peak < 1e-32 {
        return [0; 4];
    }
    let exponent = peak.log2().floor() as i32 + 1;
    let scale = 2_f32.powi(8 - exponent);
    let [r, g, b] = color.map(|channel| (channel * scale) as u8).to_array();
    [r, g, b, (exponent + 128) as u8]
}

/// Inverse of RGBE encoding, also used by CPU regressions for HDR and convolution invariants.
#[cfg(test)]
fn decode_rgbe(pixel: [u8; 4]) -> [f32; 3] {
    let scale = if pixel[3] == 0 { 0. } else { 2_f32.powi(i32::from(pixel[3]) - 136) };
    [pixel[0] as f32 * scale, pixel[1] as f32 * scale, pixel[2] as f32 * scale]
}

/// Inverse equirectangular mapping; source row zero is the north pole, longitude zero is +X.
fn direction(u: f32, v: f32) -> Color {
    let longitude = (u - 0.5) * TAU;
    let latitude = v * PI;
    Color::new(latitude.sin() * longitude.cos(), latitude.cos(), latitude.sin() * longitude.sin())
}

/// Low-discrepancy integration sequence with an exact base-two radical inverse.
fn hammersley(index: u32, count: u32) -> [f32; 2] {
    [index as f32 / count as f32, index.reverse_bits() as f32 / 4294967296.]
}

/// GGX-distributed local half vector for perceptual roughness, using alpha=roughness^2.
fn ggx_half(sample: [f32; 2], roughness: f32) -> Color {
    let alpha2 = roughness.powi(4);
    let phi = TAU * sample[0];
    let cosine = ((1. - sample[1]) / (1. + (alpha2 - 1.) * sample[1])).sqrt();
    let sine = (1. - cosine * cosine).max(0.).sqrt();
    Color::new(sine * phi.cos(), sine * phi.sin(), cosine)
}

/// Integrates separable Smith GGX visibility and Schlick Fresnel scale/bias over the hemisphere.
fn integrate_brdf(nv: f32, roughness: f32, samples: u32) -> [f32; 2] {
    let view = Color::new((1. - nv * nv).sqrt(), 0., nv);
    let mut integral = [0.; 2];
    let alpha2 = roughness.powi(4);
    let smith = |cosine: f32| 2. * cosine / (cosine + (alpha2 + (1. - alpha2) * cosine * cosine).sqrt()).max(1e-8);
    for index in 0..samples {
        let half = ggx_half(hammersley(index, samples), roughness);
        let vh = Color::dot(&view, &half).max(0.);
        let light = half * (2. * vh) - view;
        let nl = light.z.max(0.);
        if nl > 0. {
            let visibility = smith(nv) * smith(nl) * vh / (half.z * nv).max(1e-8);
            let fresnel = (1. - vh).powi(5);
            integral[0] += (1. - fresnel) * visibility / samples as f32;
            integral[1] += fresnel * visibility / samples as f32;
        }
    }
    integral
}

/// Moves the library frame's first (normal) axis to local +Z, preserving right-handedness.
/// Finite nonzero panorama directions meet the ordinary basis constructor's preconditions.
/// Cache version 2 accounts for the library's different deterministic tangent selection.
fn tangent_basis(normal: Color) -> Mat3f {
    let [normal, tangent, bitangent] = basis_from_unit_rh(&normal);
    Mat3f { col: [tangent, bitangent, normal] }
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod tests;
