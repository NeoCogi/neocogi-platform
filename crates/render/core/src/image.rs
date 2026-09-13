//! Borrowed, bounded image input used at renderer upload boundaries.

use std::{error::Error, fmt};

/// Maximum accepted byte count for one packed RGBA image.
pub const MAX_IMAGE_BYTES: usize = neocogi_renderer_contracts::MAX_IMAGE_BYTES;

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
        if self.width <= 0 || self.height <= 0 {
            return Err(ImageValidationError::DimensionsOutOfRange { width: self.width, height: self.height });
        }
        let width = self.width as usize;
        let height = self.height as usize;
        let Some(expected) = width.checked_mul(height).and_then(|pixels| pixels.checked_mul(4)) else {
            return Err(ImageValidationError::SizeOverflow { width, height });
        };
        if expected > isize::MAX as usize {
            return Err(ImageValidationError::SizeOverflow { width, height });
        }
        if expected > MAX_IMAGE_BYTES {
            return Err(ImageValidationError::TooLarge {
                width,
                height,
                required_bytes: expected,
                maximum_bytes: MAX_IMAGE_BYTES,
            });
        }
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
