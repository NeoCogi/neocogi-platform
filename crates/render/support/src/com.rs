//! Complete-frame validation shared by platform assemblies before they acquire native GPU frames.
use neocogi_renderer_contracts::*;
use neocogi_com::{ComPtr, ComRef, NcCopyToBufferOffset, NcStatus, NC_E_INVALID_ARG, NC_E_OUT_OF_MEMORY};
use std::ops::Range;

/// Concrete renderer-owned resource resolution; implementations authenticate canonical COM identities.
/// Calls happen during preparation, with no GPU frame or renderer-state borrow held.
pub trait Resources {
    /// Owned image record retaining its native device and allocation.
    type Texture;
    /// Owned application command snapshot retaining everything needed for this submission.
    type Item;
    /// Rejects resources not created by this renderer, without trusting a resource-reported owner number.
    fn texture(&self, texture: ComPtr<ITextureAbi>) -> Result<Self::Texture, NcStatus>;
    /// Authenticates a concrete application capability and snapshots its scene data before GPU execution.
    fn item(&self, item: ComPtr<IRenderItemAbi>) -> Result<Self::Item, NcStatus>;
}

/// Validated operations contain native-owned resources and no foreign callable stream.
pub enum Batch<Texture, Item> {
    /// Contiguous triangle range in PreparedFrame::vertices, already checked for finite components.
    Atlas(Range<usize>),
    /// Complete quad and its authenticated image, retained through finish or abandonment.
    Texture {
        /// Top-left, top-right, bottom-right and bottom-left vertices.
        vertices: [RenderVertex; 4],
        /// Concrete allocation owned by this renderer.
        texture: Texture,
    },
    /// Application work preserves its exact position between ordinary batches.
    Item {
        /// Unclipped content rectangle in drawable pixels.
        rect: Rect,
        /// Positive clip inside both the content rectangle and the viewport.
        clip: Rect,
        /// Concrete, owned command prepared by the co-packaged application renderer.
        item: Item,
    },
}

/// Reusable bulk-copy storage. All foreign reads and ownership checks finish before native submission.
pub struct PreparedFrame<Texture, Item> {
    /// Final validated geometry, reused across submissions.
    vertices: Vec<RenderVertex>,
    /// Pointer-free incoming records, reused rather than allocated once per frame.
    records: Vec<RenderBatch>,
    /// Ordered resolved operations, empty after failed preparation.
    batches: Vec<Batch<Texture, Item>>,
}

impl<Texture, Item> Default for PreparedFrame<Texture, Item> {
    /// Construction needs no device, resource default values, or GPU allocation.
    fn default() -> Self {
        Self {
            vertices: Vec::new(),
            records: Vec::new(),
            batches: Vec::new(),
        }
    }
}

impl<Texture, Item> PreparedFrame<Texture, Item> {
    /// Gives native execution immutable access to the complete prepared vertex buffer.
    pub fn vertices(&self) -> &[RenderVertex] {
        &self.vertices
    }
    /// Gives native execution immutable access to the validated painter-order operations.
    pub fn batches(&self) -> &[Batch<Texture, Item>] {
        &self.batches
    }
    /// Releases submitted resources after the native frame is gone, preserving bulk allocations.
    pub fn clear(&mut self) {
        self.batches.clear();
        self.vertices.clear();
        self.records.clear();
    }
    /// Validates counts and all records, then resolves resources; failure leaves no retained operations.
    /// The caller must own this storage across the call, never hold a RefCell guard while invoking it.
    pub fn read<R: Resources<Texture = Texture, Item = Item>>(
        &mut self,
        info: RenderInfo,
        stream: ComRef<'_, IRenderStreamAbi>,
        resources: &R,
    ) -> Result<(), NcStatus> {
        self.clear();
        let result = self.read_once(info, stream, resources);
        if result.is_err() {
            self.clear();
        }
        result
    }
    /// Copies and checks the complete input before resolving even the first foreign resource.
    fn read_once<R: Resources<Texture = Texture, Item = Item>>(
        &mut self,
        info: RenderInfo,
        stream: ComRef<'_, IRenderStreamAbi>,
        resources: &R,
    ) -> Result<(), NcStatus> {
        if info.invocation == 0
            || info.size.width <= 0
            || info.size.height <= 0
            || info.vertex_count > MAX_RENDER_VERTICES
            || info.batch_count > MAX_RENDER_BATCHES
        {
            return Err(NC_E_INVALID_ARG);
        }
        // Checked reservation reports allocation failure before receiver storage is published or used by a driver.
        self.vertices.try_reserve(info.vertex_count as usize).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        self.records.try_reserve(info.batch_count as usize).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        self.batches.try_reserve(info.batch_count as usize).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        self.vertices.resize(info.vertex_count as usize, RenderVertex::default());
        self.records.resize(info.batch_count as usize, RenderBatch::Atlas { first: 0, count: 0 });
        if stream.vertices(info.invocation, NcCopyToBufferOffset::new(0, &mut self.vertices))? != info.vertex_count
            || stream.batches(info.invocation, NcCopyToBufferOffset::new(0, &mut self.records))? != info.batch_count
            || self
                .vertices
                .iter()
                .any(|v| !v.x.is_finite() || !v.y.is_finite() || !v.u.is_finite() || !v.v.is_finite())
        {
            return Err(NC_E_INVALID_ARG);
        }
        let viewport = Rect {
            x: 0,
            y: 0,
            width: info.size.width,
            height: info.size.height,
        };
        let mut used = 0u32;
        for record in &self.records {
            let (first, count) = match *record {
                RenderBatch::Atlas { first, count } if count > 0 && count.is_multiple_of(3) => (first, count),
                RenderBatch::Texture { first } => (first, 4),
                RenderBatch::Item { rect, clip } if contains(rect, clip) && contains(viewport, clip) => continue,
                _ => return Err(NC_E_INVALID_ARG),
            };
            if first != used {
                return Err(NC_E_INVALID_ARG);
            }
            used = used.checked_add(count).filter(|end| *end <= info.vertex_count).ok_or(NC_E_INVALID_ARG)?;
        }
        if used != info.vertex_count {
            return Err(NC_E_INVALID_ARG);
        }
        // Only resource calls can fail from here; partial native snapshots are discarded by read() on error.
        for (index, record) in self.records.iter().enumerate() {
            self.batches.push(match *record {
                RenderBatch::Atlas { first, count } => Batch::Atlas(first as usize..(first + count) as usize),
                RenderBatch::Texture { first } => Batch::Texture {
                    vertices: self.vertices[first as usize..first as usize + 4]
                        .try_into()
                        .expect("validated four-vertex range"),
                    texture: resources.texture(stream.texture(info.invocation, index as u32)?)?,
                },
                RenderBatch::Item { rect, clip } => Batch::Item {
                    rect,
                    clip,
                    item: resources.item(stream.item(info.invocation, index as u32)?)?,
                },
            });
        }
        Ok(())
    }
}

/// Wide arithmetic checks positive rectangles without overflowing an i32 origin-plus-extent.
fn contains(outer: Rect, inner: Rect) -> bool {
    outer.width > 0
        && outer.height > 0
        && inner.width > 0
        && inner.height > 0
        && inner.x >= outer.x
        && inner.y >= outer.y
        && i64::from(inner.x) + i64::from(inner.width) <= i64::from(outer.x) + i64::from(outer.width)
        && i64::from(inner.y) + i64::from(inner.height) <= i64::from(outer.y) + i64::from(outer.height)
}

#[cfg(test)]
mod tests;
