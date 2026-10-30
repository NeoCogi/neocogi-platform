//
// Copyright 2022-Present (c) Raja Lehtihet & Wael El Oraiby
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
// -----------------------------------------------------------------------------
// Ported to rust from https://github.com/rxi/microui/ and the original license
//
// Copyright (c) 2020 rxi
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to
// deal in the Software without restriction, including without limitation the
// rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
// sell copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
// IN THE SOFTWARE.
//
//! Public renderer integration types.

use super::texture::TextureError;
use crate::{Color, TextureId};
use rs_math3d::Dimensioni;
use std::{error::Error, fmt};
/// Error returned when constructing frame metadata.
#[derive(Copy, Clone, Debug)]
pub enum FrameInfoError {
    /// A drawable frame must have positive width and height.
    NonPositiveDimensions(Dimensioni),
}

impl PartialEq for FrameInfoError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::NonPositiveDimensions(left), Self::NonPositiveDimensions(right)) => (left.width, left.height) == (right.width, right.height),
        }
    }
}

impl Eq for FrameInfoError {}

impl fmt::Display for FrameInfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositiveDimensions(dimensions) => {
                write!(f, "frame dimensions must be positive, got {}x{}", dimensions.width, dimensions.height)
            }
        }
    }
}

impl Error for FrameInfoError {}

/// Validated immutable metadata for one logical/backend frame.
#[derive(Copy, Clone)]
pub struct FrameInfo {
    dimensions: Dimensioni,
    clear: Color,
}

impl fmt::Debug for FrameInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameInfo")
            .field("width", &self.dimensions.width)
            .field("height", &self.dimensions.height)
            .field("clear", &[self.clear.r, self.clear.g, self.clear.b, self.clear.a])
            .finish()
    }
}

impl FrameInfo {
    /// Creates frame metadata after validating a positive drawable size.
    pub fn try_new(dimensions: Dimensioni, clear: Color) -> Result<Self, FrameInfoError> {
        if dimensions.width <= 0 || dimensions.height <= 0 {
            return Err(FrameInfoError::NonPositiveDimensions(dimensions));
        }
        Ok(Self { dimensions, clear })
    }

    /// Returns the drawable frame dimensions.
    pub fn dimensions(&self) -> Dimensioni {
        self.dimensions
    }

    /// Returns the frame clear color.
    pub fn clear(&self) -> Color {
        self.clear
    }
}

/// Backend-frame acquisition, command, submission, or presentation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameError {
    /// Backend-provided detail retained through renderer error propagation.
    message: String,
}

impl FrameError {
    /// Creates a backend-frame error from a displayable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl Error for FrameError {}

impl From<String> for FrameError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for FrameError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// One exclusively borrowed native target. Only explicit finish submits or presents.
/// Dropping an unfinished frame abandons its pending CPU work. Immediate APIs may already
/// have executed draws; failure is not a GPU rollback guarantee.
pub trait RendererFrame {
    /// Consume this frame and report command, submission or presentation failure.
    fn finish(self) -> Result<(), FrameError>;
}

/// Concrete current-thread backend. Generic command realization is an inherent native operation;
/// it cannot be exposed as a client callback or recovered through a public resource interface.
pub trait RendererBackend: 'static {
    /// Exclusive target borrow prevents concurrent frame acquisition in safe Rust.
    type Frame<'a>: RendererFrame
    where
        Self: 'a;
    /// Acquire and initialize the native presentation target.
    fn frame(&mut self, info: FrameInfo) -> Result<Self::Frame<'_>, FrameError>;
    /// Allocate a fresh unpublished offscreen output without CPU zero-fill.
    fn create_render_texture(&mut self, _id: TextureId) -> Result<(), TextureError> {
        Err(TextureError::backend("offscreen rendering is unavailable on this backend"))
    }
    /// Acquire an offscreen output without presentation. The host admits private UI blocks only.
    fn texture_frame(&mut self, _info: FrameInfo, _output: TextureId) -> Result<Self::Frame<'_>, FrameError> {
        Err(FrameError::new("offscreen rendering is unavailable on this backend"))
    }
    /// Upload exact validated immutable RGBA bytes for the supplied native image key.
    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError>;
    /// Retire an image key after native frames have released their references.
    fn destroy_texture(&mut self, id: TextureId);
}
