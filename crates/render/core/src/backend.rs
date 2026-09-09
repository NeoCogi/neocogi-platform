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
use crate::{Color, ImageSource, TextureId};
pub use neocogi_ui_contracts::RenderVertex as Vertex;
use rs_math3d::{Dimensioni, Rect};
use std::{error::Error, fmt};

/// Copied geometry for one owned COM render item at an ordered drawing boundary.
#[derive(Copy, Clone, Debug)]
pub struct RenderItemArgs {
    /// Dimensions of the active backend frame.
    pub dimensions: Dimensioni,
    /// Rectangle describing the widget's content area.
    pub content_area: Rect<i32>,
    /// Final visible region after operation, content-area, and viewport clipping.
    ///
    /// This value is authoritative; item implementations do not need to intersect it with `content_area`
    /// again.
    pub view: Rect<i32>,
}

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
    /// Backend-provided detail retained through Context error propagation.
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

/// Failure to upload and publish a replacement UI atlas in one renderer backend.
///
/// Theme switching uses a separate error from ordinary Context textures because the atlas is a
/// renderer-wide resource: a failed replacement must leave the previous atlas fully usable. The
/// backend-specific diagnostic is captured as owned text at this concrete public boundary, so
/// callers never need type erasure or backend-dependent generic error plumbing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AtlasUploadError {
    /// Stable display representation of the backend allocation, upload, or binding failure.
    message: String,
}

impl AtlasUploadError {
    /// Creates an atlas-upload error from one displayable backend diagnostic.
    pub fn new(message: impl Into<String>) -> Self {
        // The originating driver value is often temporary, so retain the complete diagnostic as
        // an owned string while preserving a dedicated, matchable atlas error type.
        Self { message: message.into() }
    }
}

impl fmt::Display for AtlasUploadError {
    /// Writes the backend diagnostic without adding an unrelated texture classification.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The type itself identifies the operation; the retained message preserves backend detail.
        self.message.fmt(formatter)
    }
}

impl Error for AtlasUploadError {}

impl From<String> for AtlasUploadError {
    /// Preserves an owned backend diagnostic without another formatting allocation.
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for AtlasUploadError {
    /// Copies a static or borrowed diagnostic into the independently owned public error.
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// Active-frame geometry submission interface implemented by every backend frame.
pub trait RendererFrame {
    /// Completes and consumes this frame, reporting command, submission, or presentation failure.
    ///
    /// Backends may latch an error from an earlier infallible command and return it here. Dropping
    /// an unfinished frame abandons pending work; destructors must not submit or present. Immediate
    /// APIs can already have executed work, so failure/abandonment does not promise GPU rollback.
    fn finish(self) -> Result<(), FrameError>;

    /// Appends one atlas-backed quad.
    fn push_quad(&mut self, vertices: [Vertex; 4]);
    /// Appends one atlas-backed triangle.
    fn push_triangle(&mut self, vertices: [Vertex; 3]);
    /// Appends a contiguous sequence of complete atlas-backed triangles.
    ///
    /// Buffered backends should override this method so an already prepared range can be copied
    /// once. The default preserves simpler indexed or diagnostic backends without forcing them to
    /// duplicate batching policy.
    fn push_triangles(&mut self, vertices: &[Vertex]) {
        // This native-only contract never accepts an incomplete primitive: silently ignoring a
        // remainder would hide corruption at the final renderer boundary.
        assert!(vertices.len().is_multiple_of(3), "atlas vertex range must contain complete triangles");
        for triangle in vertices.as_chunks::<3>().0 {
            self.push_triangle(*triangle);
        }
    }
    /// Closes the current atlas batch at an ordering boundary.
    fn flush(&mut self);
    /// Draws one pre-clipped external-texture quad.
    fn draw_texture(&mut self, id: TextureId, vertices: [Vertex; 4]);
    /// Transfers an owned texture to the COM submission backend. Legacy direct backends reject it.
    /// This transitional native integration hook is removed with direct Context APIs in milestone 17.
    fn draw_com_texture(&mut self, _texture: neocogi_com::ComPtr<neocogi_ui_contracts::ITextureAbi>, _vertices: [Vertex; 4]) -> Result<(), FrameError> {
        Err(FrameError::new("COM texture requires the COM runtime submission backend"))
    }
    /// Transfers owned application work without lending this native frame through COM.
    /// Direct legacy Context backends reject this transitional hook until their removal in milestone 17.
    fn draw_com_item(&mut self, _item: neocogi_com::ComPtr<neocogi_ui_contracts::IRenderItemAbi>, _args: RenderItemArgs) -> Result<(), FrameError> {
        Err(FrameError::new("COM render item requires the COM runtime submission backend"))
    }
}

/// Trait implemented by render backends used by the UI context.
///
/// Backends and their frames execute on the owning Context thread. This trait deliberately has no
/// `Send` or `Sync` bound.
///
/// Backends consume final [`Vertex`] values from Context's crate-private display-list executor:
///
/// ```
/// use neocogi_render_core::{
///     ImageSource, Color, TextureId,
///     FrameError, FrameInfo, RendererBackend, RendererFrame, TextureError, Vertex,
/// };
///
/// struct Backend {
/// }
///
/// #[must_use]
/// struct BackendFrame<'a>(&'a mut Backend);
///
/// impl RendererFrame for BackendFrame<'_> {
///     /// Completes this backend fixture; it owns no fallible native submission.
///     fn finish(self) -> Result<(), neocogi_render_core::FrameError> { Ok(()) }
///
///     fn push_quad(&mut self, _vertices: [Vertex; 4]) {}
///     fn push_triangle(&mut self, _vertices: [Vertex; 3]) {}
///     fn flush(&mut self) {}
///     fn draw_texture(&mut self, _id: TextureId, _vertices: [Vertex; 4]) {}
/// }
///
/// impl RendererBackend for Backend {
///     type Frame<'a> = BackendFrame<'a>;
///
///     fn replace_atlas(&mut self, atlas: ImageSource<'_>) -> Result<(), neocogi_render_core::AtlasUploadError> {
///         atlas.validate().map_err(|error| neocogi_render_core::AtlasUploadError::new(error.to_string()))?;
///         Ok(())
///     }
///
///     fn frame(&mut self, _info: FrameInfo) -> Result<Self::Frame<'_>, FrameError> {
///         Ok(BackendFrame(self))
///     }
///
///     fn create_texture(
///         &mut self,
///         _id: TextureId,
///         _pixels: &[u8],
///     ) -> Result<(), TextureError> {
///         Ok(())
///     }
///
///     fn destroy_texture(&mut self, _id: TextureId) {}
/// }
/// ```
///
/// An active frame owns the backend's exclusive mutable borrow, so safe Rust cannot acquire a
/// second frame until the first is dropped. The diagnostic-matched
/// `tests/ui/backend_frame_acquire_twice.rs` contract test verifies that rejection beside a passing
/// single-acquisition fixture.
pub trait RendererBackend: 'static {
    /// Exclusively borrowed active frame produced by this backend.
    type Frame<'a>: RendererFrame
    where
        Self: 'a;

    /// Uploads and publishes one replacement UI atlas transactionally.
    ///
    /// Implementations must create and populate every replacement GPU resource before discarding
    /// the currently active texture. An error leaves later frames using the previous GPU image.
    /// The borrowed pixels are not retained; this operation is called only between frames.
    fn replace_atlas(&mut self, atlas: ImageSource<'_>) -> Result<(), AtlasUploadError>;
    /// Acquires and initializes one backend frame.
    fn frame(&mut self, info: FrameInfo) -> Result<Self::Frame<'_>, FrameError>;
    /// Creates a texture owned by the backend.
    ///
    /// The caller validates [`TextureId::size`] and RGBA byte length before calling this method.
    /// Backends should return [`TextureError::backend`] without retaining `id` when GPU creation
    /// or upload fails. Image validation and identifier allocation are owned by the Context, so a
    /// backend does not manufacture those higher-level error classifications.
    /// Dimensions are carried only by `id`, preventing a backend from observing contradictory
    /// handle and argument sizes.
    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError>;
    /// Destroys a previously created texture.
    fn destroy_texture(&mut self, id: TextureId);
}
