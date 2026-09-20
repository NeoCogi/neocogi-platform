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

//! Context-owned external texture handles.

use crate::{ImageSource, ImageValidationError};
use rs_math3d::Dimensioni;
use std::{
    error::Error,
    fmt,
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

/// Concrete failure returned while validating or creating a Context-owned texture.
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
    /// This Context has successfully allocated every available texture identifier.
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

/// Concrete identity assigned once to one context-owned render executor.
///
/// Keeping this wrapper distinct from other process-unique owners prevents an atlas, retained
/// surface, or unrelated registry identity from being used as texture provenance inside the crate.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
#[doc(hidden)]
pub struct RendererId(NonZeroU64);

/// Process-wide monotonic source for renderer ownership identities.
static NEXT_RENDERER_ID: AtomicU64 = AtomicU64::new(1);

impl RendererId {
    /// Allocates the identity retained by one newly constructed Context executor.
    #[doc(hidden)]
    pub fn allocate() -> Self {
        // Relaxed ordering is sufficient: this counter establishes uniqueness but publishes no
        // object memory. Exhaustion is terminal so a destroyed renderer identity is never reused.
        let raw = NEXT_RENDERER_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
            .expect("renderer identity space exhausted");
        Self(NonZeroU64::new(raw).expect("renderer identity allocator returned zero"))
    }
}

/// Handle referencing an external texture managed by one Context.
///
/// Equality and hashing include the owning Context executor, its local allocation slot, and the
/// immutable dimensions carried by the handle. Two contexts can therefore issue the same local
/// slot without either accepting, drawing, or destroying the other's texture.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureId {
    /// Process-unique identity of the Context executor that created this texture.
    renderer: RendererId,
    /// Monotonically allocated slot meaningful only within `renderer`.
    slot: u32,
    /// Texture width in pixels.
    width: i32,
    /// Texture height in pixels.
    height: i32,
}

impl TextureId {
    /// Allocates an offscreen image key after checking dimensions without CPU pixel storage.
    pub fn for_render_target(width: i32, height: i32) -> Result<Self, TextureError> {
        // The shared validator supplies all dimension/allocation bounds. Only its
        // missing-pixel result is expected for a GPU-produced image.
        match (ImageSource { width, height, pixels: &[] }).validate() {
            Err(ImageValidationError::PixelLength { .. }) => Ok(Self::new(RendererId::allocate(), 1, width, height)),
            Err(error) => Err(error.into()),
            Ok(()) => unreachable!("a positive RGBA image requires pixel storage"),
        }
    }

    /// Allocates a native backend key after the shared image validator checks the complete upload.
    /// This key stays inside the renderer plugin; callers receive an authenticated ITexture capability.
    /// Direct Context texture allocation retains its existing owner until that API is removed in milestone 17.
    #[doc(hidden)]
    pub fn for_upload(width: i32, height: i32, pixels: &[u8]) -> Result<Self, TextureError> {
        ImageSource { width, height, pixels }.validate()?;
        Ok(Self::new(RendererId::allocate(), 1, width, height))
    }

    /// Creates one Context-owned texture capability with immutable dimensions.
    #[doc(hidden)]
    pub fn new(renderer: RendererId, slot: u32, width: i32, height: i32) -> Self {
        // Only the private executor calls this production constructor after validating dimensions
        // and before transferring the complete capability to its backend.
        Self { renderer, slot, width, height }
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

    /// Creates an isolated opaque texture capability for unit tests without a Context.
    #[doc(hidden)]
    pub fn new_test(slot: u32, width: i32, height: i32) -> Self {
        // A fresh owner prevents synthetic handles from accidentally comparing equal across tests;
        // tests that exercise equality construct several IDs from one explicit RendererId instead.
        Self::new(RendererId::allocate(), slot, width, height)
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

    /// Verifies every component of an opaque texture capability participates in identity.
    #[test]
    fn identity_includes_renderer_slot_and_immutable_dimensions() {
        let renderer = RendererId::allocate();
        let other_renderer = RendererId::allocate();
        let texture = TextureId::new(renderer, 7, 32, 16);
        let same = TextureId::new(renderer, 7, 32, 16);
        let foreign = TextureId::new(other_renderer, 7, 32, 16);
        let different_slot = TextureId::new(renderer, 8, 32, 16);
        let different_width = TextureId::new(renderer, 7, 64, 16);
        let different_height = TextureId::new(renderer, 7, 32, 8);

        assert_eq!(texture, same);
        assert_ne!(texture, foreign);
        assert_ne!(texture, different_slot);
        assert_ne!(texture, different_width);
        assert_ne!(texture, different_height);

        let textures = HashSet::from([texture]);
        assert!(textures.contains(&same));
        assert!(!textures.contains(&foreign));
        assert!(!textures.contains(&different_slot));
        assert!(!textures.contains(&different_width));
        assert!(!textures.contains(&different_height));
    }
}
