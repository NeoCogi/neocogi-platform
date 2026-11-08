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

//! Structured failures for checked native image construction.

use crate::render::ImageValidationError;
use std::{error::Error, fmt};

/// Concrete failure returned while validating or creating a native texture.
///
/// Image validation and backend creation are separate failure classes.
/// Keeping them in one enum gives every public texture-loading entry point the same inspectable
/// contract without erasing backend diagnostics behind `Any` or returning unclassified strings.
#[derive(Debug)]
pub enum TextureError {
    /// The supplied raw or compressed image violates the shared image contract.
    Image {
        /// Concrete dimension, storage, byte-count, or decoding failure.
        source: ImageValidationError,
    },
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
        // Preserve structured image diagnostics at the shared backend boundary.
        match self {
            Self::Image { source } => write!(formatter, "texture image is invalid: {source}"),
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
            Self::Backend { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies every texture failure has stable classification, diagnostics, and source behavior.
    #[test]
    fn error_contract_preserves_image_sources_without_erasing_backend_diagnostics() {
        let image = TextureError::from(ImageValidationError::PixelLength { expected: 16, actual: 4 });
        assert_eq!(image.to_string(), "texture image is invalid: expected 16 RGBA bytes, received 4");
        assert!(Error::source(&image).is_some());

        let backend = TextureError::backend("device rejected upload");
        assert_eq!(backend.to_string(), "backend texture creation failed: device rejected upload");
        assert!(matches!(&backend, TextureError::Backend { message } if message == "device rejected upload"));
        assert!(Error::source(&backend).is_none());
    }
}
