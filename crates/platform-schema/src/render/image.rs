//! Borrowed, bounded image input used at renderer upload boundaries.

use std::{error::Error, fmt};

/// Maximum accepted byte count for one packed RGBA image.
pub const MAX_IMAGE_BYTES: usize = neocogi_platform_schema::render::abi::MAX_IMAGE_BYTES;

/// Checks positive RGBA dimensions and returns their exact addressable, bounded storage size.
/// Render-target allocation uses this without fabricating an empty CPU upload.
pub fn checked_rgba_len(width: i32, height: i32) -> Result<usize, ImageValidationError> {
    if width <= 0 || height <= 0 {
        return Err(ImageValidationError::DimensionsOutOfRange { width, height });
    }
    let width = width as usize;
    let height = height as usize;
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or(ImageValidationError::SizeOverflow { width, height })?;
    if expected > MAX_IMAGE_BYTES {
        return Err(ImageValidationError::TooLarge {
            width,
            height,
            required_bytes: expected,
            maximum_bytes: MAX_IMAGE_BYTES,
        });
    }
    Ok(expected)
}

/// Borrowed packed RGBA pixels with explicit signed dimensions.
#[derive(Copy, Clone, Debug)]
pub struct ImageSource<'a> {
    /// Positive width in pixels, checked before allocation or upload.
    pub width: i32,
    /// Positive height in pixels, checked before allocation or upload.
    pub height: i32,
    /// Exactly width × height × 4 bytes in RGBA8888 channel order.
    pub pixels: &'a [u8],
}

impl ImageSource<'_> {
    /// Checks dimensions, addressable size, the shared allocation bound, and exact byte count.
    pub fn validate(self) -> Result<(), ImageValidationError> {
        let expected = checked_rgba_len(self.width, self.height)?;
        if self.pixels.len() != expected {
            return Err(ImageValidationError::PixelLength { expected, actual: self.pixels.len() });
        }
        Ok(())
    }
}

/// Concrete validation failure returned before a renderer may retain or upload image bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageValidationError {
    /// One or both signed dimensions are not positive.
    DimensionsOutOfRange {
        /// Supplied width.
        width: i32,
        /// Supplied height.
        height: i32,
    },
    /// Dimension arithmetic exceeds the addressable allocation domain.
    SizeOverflow {
        /// Positive width converted to the platform allocation type.
        width: usize,
        /// Positive height converted to the platform allocation type.
        height: usize,
    },
    /// The valid extent exceeds the platform's shared image budget.
    TooLarge {
        /// Positive width in pixels.
        width: usize,
        /// Positive height in pixels.
        height: usize,
        /// Exact byte count required by the image.
        required_bytes: usize,
        /// Maximum accepted packed byte count.
        maximum_bytes: usize,
    },
    /// The borrowed slice does not exactly fill its declared extent.
    PixelLength {
        /// Required packed RGBA byte count.
        expected: usize,
        /// Supplied byte count.
        actual: usize,
    },
}

impl fmt::Display for ImageValidationError {
    /// Formats every value needed to correct malformed upload input.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionsOutOfRange { width, height } => {
                write!(formatter, "image dimensions must be positive, received {width}x{height}")
            }
            Self::SizeOverflow { width, height } => {
                write!(formatter, "image dimensions {width}x{height} overflow addressable RGBA storage")
            }
            Self::TooLarge {
                width,
                height,
                required_bytes,
                maximum_bytes,
            } => write!(
                formatter,
                "image dimensions {width}x{height} require {required_bytes} bytes, exceeding the {maximum_bytes}-byte limit"
            ),
            Self::PixelLength { expected, actual } => {
                write!(formatter, "expected {expected} RGBA bytes, received {actual}")
            }
        }
    }
}

impl Error for ImageValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// COM maps every structured extent failure to InvalidArg without a second policy body.
    #[test]
    fn contract_extent_statuses_preserve_native_acceptance_boundaries() {
        use crate::render::abi::{Size, rgba_bytes};
        for (width, height, expected) in [
            (1, 1, Some(4)),
            (8192, 2048, Some(64 * 1024 * 1024)),
            (8193, 2048, None),
            (0, 1, None),
            (1, -1, None),
            (i32::MAX, i32::MAX, None),
            (i32::MAX, 1, None),
        ] {
            let native = checked_rgba_len(width, height);
            let contract = rgba_bytes(Size { width, height });
            match expected {
                Some(bytes) => {
                    assert_eq!(native.unwrap(), bytes);
                    assert_eq!(contract.unwrap(), bytes);
                }
                None => {
                    assert!(native.is_err());
                    assert_eq!(contract, Err(neocogi_com::NC_E_INVALID_ARG));
                }
            }
        }
    }

    /// Extent-only allocation and pixel admission share arithmetic, but only uploads require bytes.
    #[test]
    fn extent_and_actual_pixel_admission_have_explicit_boundaries() {
        assert_eq!(checked_rgba_len(1, 1), Ok(4));
        for (width, height) in [(0, 1), (-1, 1), (1, 0), (1, -1)] {
            assert_eq!(
                checked_rgba_len(width, height),
                Err(ImageValidationError::DimensionsOutOfRange { width, height })
            );
        }
        assert!(matches!(checked_rgba_len(i32::MAX, i32::MAX), Err(ImageValidationError::SizeOverflow { .. })));
        assert!(matches!(
            checked_rgba_len(i32::MAX, 1),
            Err(ImageValidationError::TooLarge { .. }) | Err(ImageValidationError::SizeOverflow { .. })
        ));
        for len in [0, 1, 3, 5] {
            let bytes = vec![0; len];
            assert_eq!(
                ImageSource { width: 1, height: 1, pixels: &bytes }.validate(),
                Err(ImageValidationError::PixelLength { expected: 4, actual: len })
            );
        }
        assert_eq!(ImageSource { width: 1, height: 1, pixels: &[0; 4] }.validate(), Ok(()));
    }
}
