//! Checked viewport/clip geometry, independent of pipelines and backend state.
use crate::gpu::abi::{Rect, RenderViewInfo, RenderViewRoot, Size};
use neocogi_com::{NcStatus, NC_E_INVALID_ARG, NC_E_OUT_OF_MEMORY, NC_E_OUT_OF_RANGE};

/// One flattened logical state; clip changes never refit the viewport mapping.
#[derive(Clone, Copy, Debug)]
pub struct ViewState {
    /// Current viewport in immutable-root-local coordinates.
    pub viewport: Rect,
    /// Effective raster clip in the same coordinates.
    pub clip: Rect,
}

/// Single authoritative stack for client viewport and clip operations.
pub struct ViewStack {
    /// Host-selected final visible client, never modified by client commands.
    root: RenderViewRoot,
    /// Positive maximum depth including root; allocation is bounded at construction.
    limit: usize,
    /// Exactly one saved complete state per successful push.
    states: Vec<ViewState>,
}

impl ViewStack {
    /// Validates a visible host root and reserves the entire bounded stack.
    pub fn new(root: RenderViewRoot, limit: u32) -> Result<Self, NcStatus> {
        if limit == 0
            || limit > 64
            || root.client.width < 0
            || root.client.height < 0
            || root.client.width != root.target.width
            || root.client.height != root.target.height
            || root.target.x < 0
            || root.target.y < 0
            || i64::from(root.target.x) + i64::from(root.target.width) > i64::from(i32::MAX)
            || i64::from(root.target.y) + i64::from(root.target.height) > i64::from(i32::MAX)
        {
            return Err(NC_E_INVALID_ARG);
        }
        let mut states = Vec::new();
        states.try_reserve_exact(limit as usize).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        let bounds = Rect {
            x: 0,
            y: 0,
            width: root.target.width,
            height: root.target.height,
        };
        states.push(ViewState { viewport: bounds, clip: bounds });
        Ok(Self { root, limit: limit as usize, states })
    }

    /// Immutable host root retained in sealed block provenance.
    pub fn root(&self) -> RenderViewRoot {
        self.root
    }

    /// Complete current flattened state, not a reference into a mutable stack.
    pub fn current(&self) -> ViewState {
        *self.states.last().expect("root cannot be removed")
    }

    /// Number of states including root, used to require balanced finish.
    pub fn depth(&self) -> usize {
        self.states.len()
    }

    /// Copied mapping/visibility information for camera and pointer calculations.
    pub fn info(&self) -> RenderViewInfo {
        let state = self.current();
        RenderViewInfo {
            client: self.root.client,
            viewport: state.viewport,
            clip: state.clip,
            bounds: Rect {
                x: 0,
                y: 0,
                width: state.viewport.width,
                height: state.viewport.height,
            },
        }
    }

    /// Pushes a fully contained requested viewport, intersecting visibility separately.
    pub fn push_view(&mut self, local: Rect) -> Result<RenderViewInfo, NcStatus> {
        self.capacity()?;
        let parent = self.current();
        if !contains(
            Rect {
                x: 0,
                y: 0,
                width: parent.viewport.width,
                height: parent.viewport.height,
            },
            local,
        ) {
            return Err(NC_E_INVALID_ARG);
        }
        let viewport = Rect {
            x: parent.viewport.x.checked_add(local.x).ok_or(NC_E_OUT_OF_RANGE)?,
            y: parent.viewport.y.checked_add(local.y).ok_or(NC_E_OUT_OF_RANGE)?,
            width: local.width,
            height: local.height,
        };
        self.states.push(ViewState {
            viewport,
            clip: intersection(viewport, parent.clip),
        });
        Ok(self.info())
    }

    /// Narrows visibility through wide arithmetic, preserving the current projection/input mapping.
    pub fn push_clip(&mut self, local: Rect) -> Result<(), NcStatus> {
        self.capacity()?;
        if local.width < 0 || local.height < 0 {
            return Err(NC_E_INVALID_ARG);
        }
        let parent = self.current();
        let left = i64::from(parent.viewport.x) + i64::from(local.x);
        let top = i64::from(parent.viewport.y) + i64::from(local.y);
        let right = left + i64::from(local.width);
        let bottom = top + i64::from(local.height);
        let bounds = intersection(parent.viewport, parent.clip);
        let clip = wide_intersection(bounds, left, top, right, bottom);
        self.states.push(ViewState { viewport: parent.viewport, clip });
        Ok(())
    }

    /// Restores a preceding complete state; failed underflow leaves root untouched.
    pub fn pop(&mut self) -> Result<(), NcStatus> {
        if self.states.len() == 1 {
            return Err(NC_E_INVALID_ARG);
        }
        self.states.pop();
        Ok(())
    }

    /// Restores root mapping/clip only; pipelines, recorded commands and attachment depth are separate.
    pub fn reset(&mut self) {
        self.states.truncate(1);
    }

    /// Converts a bounded root-local state to target coordinates using proven root containment.
    pub fn target(&self, local: Rect) -> Result<Rect, NcStatus> {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: self.root.target.width,
            height: self.root.target.height,
        };
        if !contains(bounds, local) {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(Rect {
            x: self.root.target.x.checked_add(local.x).ok_or(NC_E_OUT_OF_RANGE)?,
            y: self.root.target.y.checked_add(local.y).ok_or(NC_E_OUT_OF_RANGE)?,
            width: local.width,
            height: local.height,
        })
    }

    /// Charges are owned by the recorder; this helper only prevents bounded storage growth.
    fn capacity(&self) -> Result<(), NcStatus> {
        if self.states.len() >= self.limit { Err(NC_E_OUT_OF_RANGE) } else { Ok(()) }
    }
}

/// Inclusive containment for empty states, using mathematical rather than wrapping edges.
pub fn contains(outer: Rect, inner: Rect) -> bool {
    outer.width >= 0
        && outer.height >= 0
        && inner.width >= 0
        && inner.height >= 0
        && i64::from(inner.x) >= i64::from(outer.x)
        && i64::from(inner.y) >= i64::from(outer.y)
        && i64::from(inner.x) + i64::from(inner.width) <= i64::from(outer.x) + i64::from(outer.width)
        && i64::from(inner.y) + i64::from(inner.height) <= i64::from(outer.y) + i64::from(outer.height)
}

/// Intersects valid nonnegative rectangles, returning an empty point inside the first rectangle.
pub fn intersection(outer: Rect, inner: Rect) -> Rect {
    wide_intersection(
        outer,
        i64::from(inner.x),
        i64::from(inner.y),
        i64::from(inner.x) + i64::from(inner.width),
        i64::from(inner.y) + i64::from(inner.height),
    )
}

/// Restricts wide caller edges before narrowing; an empty result never expands a viewport.
fn wide_intersection(bounds: Rect, left: i64, top: i64, right: i64, bottom: i64) -> Rect {
    let bound_left = i64::from(bounds.x);
    let bound_top = i64::from(bounds.y);
    let bound_right = bound_left + i64::from(bounds.width);
    let bound_bottom = bound_top + i64::from(bounds.height);
    let left = left.max(bound_left).min(bound_right);
    let top = top.max(bound_top).min(bound_bottom);
    let right = right.min(bound_right).max(left);
    let bottom = bottom.min(bound_bottom).max(top);
    Rect {
        x: left as i32,
        y: top as i32,
        width: (right - left) as i32,
        height: (bottom - top) as i32,
    }
}

/// Top-left physical pixel rectangle shared by all native adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelRect {
    /// Inclusive left edge.
    pub x: u32,
    /// Inclusive top edge.
    pub y: u32,
    /// Nonnegative horizontal pixel extent.
    pub width: u32,
    /// Nonnegative vertical pixel extent.
    pub height: u32,
}

/// Converts clipped logical edges with exact outward rounding, independently on both axes.
pub fn physical_rect(rect: Rect, logical: Size, physical: Size) -> Result<Option<PixelRect>, NcStatus> {
    if logical.width <= 0 || logical.height <= 0 || physical.width <= 0 || physical.height <= 0 || rect.width < 0 || rect.height < 0 {
        return Err(NC_E_INVALID_ARG);
    }
    if rect.width == 0 || rect.height == 0 {
        return Ok(None);
    }
    let span = |origin: i32, extent: i32, logical: i32, pixels: i32| {
        let limit = i64::from(logical);
        let near = i64::from(origin).clamp(0, limit);
        let far = (i64::from(origin) + i64::from(extent)).clamp(0, limit);
        (
            (near * i64::from(pixels) / limit) as u32,
            ((far * i64::from(pixels) + limit - 1) / limit) as u32,
        )
    };
    let (left, right) = span(rect.x, rect.width, logical.width, physical.width);
    let (top, bottom) = span(rect.y, rect.height, logical.height, physical.height);
    Ok((right > left && bottom > top).then_some(PixelRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }))
}

#[cfg(test)]
mod tests;
