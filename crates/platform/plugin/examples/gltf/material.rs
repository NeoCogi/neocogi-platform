//! Material packing belongs to the example, preserving the renderer's four sampled-slot limit.
//! Four full-resolution tiles share one atlas; AO occupies the unused metallic/roughness red
//! channel. Shader-side bilinear sampling repeats within each tile, preventing cross-tile bleed.
use super::scene::{Image, Material};

/// Four explicit material rectangles follow the four existing material parameter vec4 values.
pub const UNIFORM_BYTES: u32 = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Integer tile in a top-left-origin RGBA8 atlas, preserving original image dimensions.
pub struct Tile {
    /// First column.
    pub x: u32,
    /// First row.
    pub y: u32,
    /// Positive width.
    pub width: u32,
    /// Positive height.
    pub height: u32,
}

/// One immutable material upload; no map is rescaled to make room for environment lighting.
pub struct MaterialAtlas {
    /// Positive complete width in pixels.
    pub width: u32,
    /// Positive complete height in pixels.
    pub height: u32,
    /// Exactly width * height * 4 initialized bytes.
    pub pixels: Vec<u8>,
    /// Base, ORM, normal and emissive tiles in shader order.
    pub tiles: [Tile; 4],
}

impl MaterialAtlas {
    /// Resolves image references and constructs all regions before GPU publication.
    pub fn prepare(material: &Material, images: &[Image]) -> Result<Self, String> {
        let lookup = |index: Option<usize>| match index {
            Some(index) => images.get(index).map(Some).ok_or("material image index is out of range"),
            None => Ok(None),
        };
        let maps = material.images.map(lookup);
        let [base, mr, normal, emissive] = maps;
        let maps = [base?, mr?, normal?, emissive?];
        let occlusion = lookup(material.occlusion)?;
        if !material.occlusion_strength.is_finite() || !(0.0..=1.0).contains(&material.occlusion_strength) {
            return Err("invalid material occlusion strength".into());
        }
        for image in maps.iter().copied().chain([occlusion]).flatten() {
            if image.width == 0 || image.height == 0 || rgba_len(image.width, image.height) != Some(image.pixels.len() as u64) {
                return Err("invalid material image extent".into());
            }
        }
        let shapes = std::array::from_fn::<_, 4, _>(|index| {
            maps[index]
                .or(if index == 1 { occlusion } else { None })
                .map_or((1, 1), |image| (image.width, image.height))
        });
        let left = shapes[0].0.max(shapes[2].0);
        let right = shapes[1].0.max(shapes[3].0);
        let top = shapes[0].1.max(shapes[1].1);
        let bottom = shapes[2].1.max(shapes[3].1);
        let width = left.checked_add(right).ok_or("material atlas width overflow")?;
        let height = top.checked_add(bottom).ok_or("material atlas height overflow")?;
        let byte_len = rgba_len(width, height).ok_or("material atlas exceeds the portable 64 MiB image budget")?;
        let tiles = std::array::from_fn(|index| Tile {
            x: if index % 2 == 0 { 0 } else { left },
            y: if index < 2 { 0 } else { top },
            width: shapes[index].0,
            height: shapes[index].1,
        });
        let mut atlas = Self {
            width,
            height,
            pixels: vec![0; byte_len as usize],
            tiles,
        };
        let defaults = [[255; 4], [255; 4], [128, 128, 255, 255], [0, 0, 0, 255]];
        for (index, tile) in tiles.into_iter().enumerate() {
            for y in 0..tile.height {
                for x in 0..tile.width {
                    let mut pixel = maps[index].map_or(defaults[index], |image| texel(image, x, y));
                    if index == 1 {
                        let ao = occlusion.map_or(1., |image| {
                            if image.width == tile.width && image.height == tile.height {
                                f32::from(texel(image, x, y)[0]) / 255.
                            } else {
                                sample_red(image, (x as f32 + 0.5) / tile.width as f32, (y as f32 + 0.5) / tile.height as f32)
                            }
                        });
                        pixel[0] = ((1. + material.occlusion_strength * (ao - 1.)) * 255.).round() as u8;
                    }
                    let offset = (((tile.y + y) * width + tile.x + x) * 4) as usize;
                    atlas.pixels[offset..offset + 4].copy_from_slice(&pixel);
                }
            }
        }
        Ok(atlas)
    }
}

/// Bounds all source and atlas arithmetic before indexing initialized image storage.
fn rgba_len(width: u32, height: u32) -> Option<u64> {
    u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(4)
        .filter(|bytes| *bytes <= 64 * 1024 * 1024)
}

/// Reads a validated tightly packed image; callers supply coordinates inside the image.
fn texel(image: &Image, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * image.width + x) * 4) as usize;
    image.pixels[offset..offset + 4].try_into().expect("validated RGBA8 image")
}

/// Resamples only AO when its resolution differs from metallic/roughness, in linear space.
fn sample_red(image: &Image, u: f32, v: f32) -> f32 {
    let x = u * image.width as f32 - 0.5;
    let y = v * image.height as f32 - 0.5;
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - x.floor();
    let fy = y - y.floor();
    let mut result = 0.;
    for dy in 0..2 {
        for dx in 0..2 {
            let px = (ix + dx).rem_euclid(image.width as i32) as u32;
            let py = (iy + dy).rem_euclid(image.height as i32) as u32;
            let weight = if dx == 0 { 1. - fx } else { fx } * if dy == 0 { 1. - fy } else { fy };
            result += f32::from(texel(image, px, py)[0]) / 255. * weight;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Default opaque material with no image dependencies.
    fn material() -> Material {
        Material {
            base: [1.; 4],
            emissive: [0.; 3],
            metal_rough: [1.; 2],
            normal_scale: 1.,
            images: [None; 4],
            occlusion: None,
            occlusion_strength: 1.,
        }
    }
    /// Single-texel images distinguish every tile's color/data interpretation.
    fn image(pixel: [u8; 4]) -> Image {
        Image {
            width: 1,
            height: 1,
            pixels: pixel.to_vec(),
        }
    }
    /// Authored pixels retain resolution and channel identity while AO occupies ORM's red.
    #[test]
    fn packs_all_maps_and_occlusion_without_rescaling() {
        let images = vec![
            image([10, 20, 30, 255]),
            image([80, 90, 100, 255]),
            image([128, 128, 255, 255]),
            image([5, 6, 7, 255]),
            image([0, 0, 0, 255]),
        ];
        let mut material = material();
        material.images = [Some(0), Some(1), Some(2), Some(3)];
        material.occlusion = Some(4);
        material.occlusion_strength = 0.5;
        let atlas = MaterialAtlas::prepare(&material, &images).unwrap();
        assert_eq!((atlas.width, atlas.height), (2, 2));
        assert_eq!(atlas.pixels, [10, 20, 30, 255, 128, 90, 100, 255, 128, 128, 255, 255, 5, 6, 7, 255]);
        for tile in atlas.tiles {
            assert_eq!((tile.width, tile.height), (1, 1));
        }
    }
    /// Missing maps use explicit initialized defaults; invalid storage and references reject.
    #[test]
    fn defaults_and_validation_are_explicit() {
        let mut material = material();
        assert_eq!(
            MaterialAtlas::prepare(&material, &[]).unwrap().pixels,
            [255, 255, 255, 255, 255, 255, 255, 255, 128, 128, 255, 255, 0, 0, 0, 255]
        );
        material.images[0] = Some(1);
        assert!(MaterialAtlas::prepare(&material, &[]).is_err());
        material.images[0] = Some(0);
        let broken = Image { width: 2, height: 2, pixels: vec![0; 4] };
        assert!(MaterialAtlas::prepare(&material, &[broken]).is_err());
    }
    /// AO-only materials keep their resolution, and mixed resolutions use linear AO resampling.
    #[test]
    fn ao_only_and_mixed_resolutions_are_supported() {
        let images = vec![
            image([255; 4]),
            Image {
                width: 2,
                height: 1,
                pixels: vec![0, 0, 0, 255, 255, 255, 255, 255],
            },
        ];
        let mut material = material();
        material.occlusion = Some(1);
        let atlas = MaterialAtlas::prepare(&material, &images).unwrap();
        assert_eq!(atlas.tiles[1].width, 2);
        material.images[1] = Some(0);
        let atlas = MaterialAtlas::prepare(&material, &images).unwrap();
        let tile = atlas.tiles[1];
        assert_eq!(atlas.pixels[((tile.y * atlas.width + tile.x) * 4) as usize], 128);
    }
}
