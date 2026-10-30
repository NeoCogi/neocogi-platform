//
// Copyright 2023-Present (c) Raja Lehtihet & Wael El Oraiby
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice,
// this list of conditions and the following disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice,
// this list of conditions and the following disclaimer in the documentation
// and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors
// may be used to endorse or promote products derived from this software without
// specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
// AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
// LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
// CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
// SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
// INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
// CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
// ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
// POSSIBILITY OF SUCH DAMAGE.
//

//! Process-unique native texture keys and structured upload failures.

use crate::{ImageSource, ImageValidationError, checked_rgba_len};
use rs_math3d::Dimensioni;
use std::{
    error::Error,
    fmt,
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

/// Concrete failure returned while validating or creating a native texture.
///
/// Image validation, identifier allocation, and backend creation are separate failure classes.
/// Keeping them in one enum gives every public texture-loading entry point the same inspectable
/// contract without erasing backend diagnostics behind `Any` or returning unclassified strings.
#[derive(Debug)]
pub enum TextureError {
    /// The supplied raw or compressed image violates the shared image contract.
    Image {
        /// Concrete dimension, storage, byte-count, or decoding failure.
        source: ImageValidationError,
    },
    /// The process has allocated every available native texture identifier.
    IdentifierSpaceExhausted,
    /// The rendering backend could not create or upload the validated texture.
    Backend {
        /// Stable backend diagnostic captured at the public error boundary.
        message: String,
    },
}

impl TextureError {
    /// Classifies one backend-specific diagnostic as a texture-creation failure.
    ///
    /// Backends can pass any concrete displayable driver, allocation, or device error. The
    /// diagnostic is formatted immediately so this public error remains one concrete type instead
    /// of exposing a backend-dependent generic or a type-erased source object.
    pub fn backend(diagnostic: impl fmt::Display) -> Self {
        // The backend boundary needs the diagnostic after the originating temporary has gone out
        // of scope, so retain its complete display representation as owned text.
        Self::Backend { message: diagnostic.to_string() }
    }
}

impl From<ImageValidationError> for TextureError {
    /// Retains an image-layer failure as the texture error's concrete source.
    fn from(source: ImageValidationError) -> Self {
        // Composition keeps all structured image fields available to callers and avoids repeating
        // image validation variants in the texture vocabulary.
        Self::Image { source }
    }
}

impl fmt::Display for TextureError {
    /// Formats the failure class together with its concrete image or backend diagnostic.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Prefix composed errors at this abstraction boundary while keeping identifier exhaustion
        // independent of the executor's private slot representation.
        match self {
            Self::Image { source } => write!(formatter, "texture image is invalid: {source}"),
            Self::IdentifierSpaceExhausted => formatter.write_str("texture identifier space is exhausted"),
            Self::Backend { message } => write!(formatter, "backend texture creation failed: {message}"),
        }
    }
}

impl Error for TextureError {
    /// Exposes the structured image cause; textual backend diagnostics have no erased source.
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        // Only Image owns another concrete standard error. Backend diagnostics intentionally stay
        // as classified text rather than entering an `Any`-based downcast protocol.
        match self {
            Self::Image { source } => Some(source),
            Self::IdentifierSpaceExhausted | Self::Backend { .. } => None,
        }
    }
}

/// Process-wide monotonic keys; exhaustion rejects rather than reusing a live or retired identity.
static NEXT_TEXTURE_ID: AtomicU64 = AtomicU64::new(1);

/// Allocates once from the supplied sequence without publishing memory or wrapping.
fn allocate(counter: &AtomicU64) -> Result<NonZeroU64, TextureError> {
    let raw = counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
        .map_err(|_| TextureError::IdentifierSpaceExhausted)?;
    Ok(NonZeroU64::new(raw).expect("texture key sequence starts at one"))
}

/// Process-unique native map key with immutable, admitted dimensions.
/// This is not a COM capability or proof that any later upload slice has the right length.
/// Device/provenance authentication belongs to the renderer's public COM resources.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureId {
    /// Never reused, including after a failed native creation or texture destruction.
    key: NonZeroU64,
    /// Positive, bounded width in pixels.
    width: i32,
    /// Positive, bounded height in pixels.
    height: i32,
}

impl TextureId {
    /// Allocates an offscreen image key after checking extent without requiring CPU pixels.
    pub fn for_render_target(width: i32, height: i32) -> Result<Self, TextureError> {
        checked_rgba_len(width, height)?;
        Ok(Self {
            key: allocate(&NEXT_TEXTURE_ID)?,
            width,
            height,
        })
    }

    /// Allocates a key for admitted pixels; each later upload must validate its own slice again.
    pub fn for_upload(width: i32, height: i32, pixels: &[u8]) -> Result<Self, TextureError> {
        ImageSource { width, height, pixels }.validate()?;
        Ok(Self {
            key: allocate(&NEXT_TEXTURE_ID)?,
            width,
            height,
        })
    }

    /// Returns the texture width in pixels.
    pub fn width(self) -> i32 {
        self.width
    }

    /// Returns the texture height in pixels.
    pub fn height(self) -> i32 {
        self.height
    }

    /// Returns the texture dimensions in pixels.
    pub fn size(self) -> Dimensioni {
        Dimensioni::new(self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// GPU-produced images obey the same allocation limits as uploaded images.
    #[test]
    fn render_target_rejects_invalid_extents_without_pixel_allocation() {
        let id = TextureId::for_render_target(64, 32).unwrap();
        assert_eq!((id.width(), id.height()), (64, 32));
        assert!(matches!(TextureId::for_render_target(0, 32), Err(TextureError::Image { .. })));
        assert!(matches!(TextureId::for_render_target(-1, 32), Err(TextureError::Image { .. })));
        assert!(matches!(TextureId::for_render_target(i32::MAX, i32::MAX), Err(TextureError::Image { .. })));
    }

    /// Verifies every texture failure has stable classification, diagnostics, and source behavior.
    #[test]
    fn error_contract_preserves_image_sources_without_erasing_backend_diagnostics() {
        let image = TextureError::from(ImageValidationError::PixelLength { expected: 16, actual: 4 });
        assert_eq!(image.to_string(), "texture image is invalid: expected 16 RGBA bytes, received 4");
        assert!(Error::source(&image).is_some());

        let exhausted = TextureError::IdentifierSpaceExhausted;
        assert_eq!(exhausted.to_string(), "texture identifier space is exhausted");
        assert!(Error::source(&exhausted).is_none());

        let backend = TextureError::backend("device rejected upload");
        assert_eq!(backend.to_string(), "backend texture creation failed: device rejected upload");
        assert!(matches!(&backend, TextureError::Backend { message } if message == "device rejected upload"));
        assert!(Error::source(&backend).is_none());
    }

    /// Separately issued keys never alias, while copying preserves the same map identity.
    #[test]
    fn keys_are_unique_and_exhaustion_never_wraps() {
        let first = TextureId::for_render_target(1, 1).unwrap();
        let second = TextureId::for_upload(1, 1, &[0; 4]).unwrap();
        assert_ne!(first, second);
        let keys = HashSet::from([first]);
        assert!(keys.contains(&first));
        assert!(!keys.contains(&second));
        let sequence = AtomicU64::new(u64::MAX - 1);
        assert_eq!(allocate(&sequence).unwrap().get(), u64::MAX - 1);
        assert!(matches!(allocate(&sequence), Err(TextureError::IdentifierSpaceExhausted)));
        assert!(matches!(allocate(&sequence), Err(TextureError::IdentifierSpaceExhausted)));
        assert_eq!(sequence.load(Ordering::Relaxed), u64::MAX);
    }
}
