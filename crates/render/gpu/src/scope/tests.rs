//! Geometry fixtures compare projection and clip independently and preserve wide-edge regressions.
use super::*;

/// Stable component comparison without assuming generated record equality/layout.
fn tuple(rect: Rect) -> (i32, i32, i32, i32) {
    (rect.x, rect.y, rect.width, rect.height)
}

/// Typical translated panel root, with an unrelated widget-local client origin.
fn root() -> RenderViewRoot {
    RenderViewRoot {
        client: Rect { x: 5, y: 7, width: 200, height: 120 },
        target: Rect { x: 100, y: 80, width: 200, height: 120 },
    }
}

/// A clip cannot alter the requested child camera aspect or pointer normalization.
#[test]
fn cropped_child_preserves_viewport_mapping_and_reports_clip_separately() {
    let rect = Rect { x: 0, y: 0, width: 100, height: 100 };
    let mut stack = ViewStack::new(RenderViewRoot { client: rect, target: rect }, 8).unwrap();
    stack.push_clip(Rect { x: 50, y: 0, width: 50, height: 100 }).unwrap();
    let info = stack.push_view(Rect { x: 20, y: 0, width: 80, height: 100 }).unwrap();
    assert_eq!(tuple(info.viewport), (20, 0, 80, 100));
    assert_eq!(tuple(info.clip), (50, 0, 50, 100));
    assert_eq!(tuple(info.bounds), (0, 0, 80, 100));
    assert_eq!(50 - info.client.x - info.viewport.x, 30);
    assert_eq!(info.bounds.width as f32 / info.bounds.height as f32, 0.8);
    stack.pop().unwrap();
    assert_eq!(tuple(stack.info().viewport), (0, 0, 100, 100));
    assert_eq!(tuple(stack.info().clip), (50, 0, 50, 100));
    stack.pop().unwrap();
    assert_eq!(tuple(stack.info().clip), (0, 0, 100, 100));
}

/// Translation belongs to the host; broad clips and reset cannot reach the window.
#[test]
fn translated_nested_view_and_clip_remain_inside_immutable_root() {
    let mut stack = ViewStack::new(root(), 8).unwrap();
    stack
        .push_clip(Rect {
            x: -50,
            y: -50,
            width: 1000,
            height: 1000,
        })
        .unwrap();
    assert_eq!(tuple(stack.target(stack.current().clip).unwrap()), (100, 80, 200, 120));
    stack.push_view(Rect { x: 20, y: 10, width: 80, height: 60 }).unwrap();
    stack.push_clip(Rect { x: 10, y: 5, width: 20, height: 15 }).unwrap();
    assert_eq!(tuple(stack.target(stack.current().viewport).unwrap()), (120, 90, 80, 60));
    assert_eq!(tuple(stack.target(stack.current().clip).unwrap()), (130, 95, 20, 15));
    stack.reset();
    assert_eq!(stack.depth(), 1);
    assert_eq!(tuple(stack.target(stack.current().clip).unwrap()), (100, 80, 200, 120));
}

/// Underflow/overflow/oversized view errors leave the previous complete state unchanged.
#[test]
fn stack_errors_do_not_mutate_state_or_remove_root() {
    let mut stack = ViewStack::new(root(), 2).unwrap();
    assert!(stack.pop().is_err());
    assert!(stack.push_view(Rect { x: -1, y: 0, width: 1, height: 1 }).is_err());
    assert!(stack.push_view(Rect { x: 0, y: 0, width: 201, height: 120 }).is_err());
    assert_eq!(stack.depth(), 1);
    stack.push_view(Rect { x: 0, y: 0, width: 200, height: 120 }).unwrap();
    assert!(stack.push_clip(Rect { x: 0, y: 0, width: 1, height: 1 }).is_err());
    assert_eq!(stack.depth(), 2);
    stack.pop().unwrap();
    assert_eq!(tuple(stack.info().bounds), (0, 0, 200, 120));
}

/// Extreme signed clip origins/edges are intersected before narrowing into root-local values.
#[test]
fn every_extreme_clip_is_contained_and_reset_restores_root() {
    let mut stack = ViewStack::new(root(), 4).unwrap();
    let bounds = Rect { x: 0, y: 0, width: 200, height: 120 };
    for x in [i32::MIN, -201, -1, 0, 1, 199, 200, i32::MAX] {
        for y in [i32::MIN, -121, 0, 1, 119, 120, i32::MAX] {
            for width in [0, 1, 200, i32::MAX] {
                for height in [0, 1, 120, i32::MAX] {
                    stack.push_clip(Rect { x, y, width, height }).unwrap();
                    assert!(contains(bounds, stack.current().clip));
                    assert!(contains(stack.current().viewport, stack.current().clip));
                    stack.reset();
                    assert_eq!(tuple(stack.current().clip), tuple(bounds));
                }
            }
        }
    }
}

/// Empty roots/children produce no physical pixels and do not expose a divide-by-zero mapping.
#[test]
fn empty_views_and_invisible_children_stay_empty() {
    let empty = Rect { x: 0, y: 0, width: 0, height: 120 };
    let mut stack = ViewStack::new(RenderViewRoot { client: empty, target: empty }, 4).unwrap();
    stack
        .push_clip(Rect {
            x: i32::MIN,
            y: i32::MIN,
            width: i32::MAX,
            height: i32::MAX,
        })
        .unwrap();
    assert_eq!(stack.info().bounds.width, 0);
    assert!(
        physical_rect(stack.current().clip, Size { width: 200, height: 120 }, Size { width: 300, height: 180 })
            .unwrap()
            .is_none()
    );
    let mut stack = ViewStack::new(root(), 4).unwrap();
    stack.push_clip(Rect { x: 0, y: 0, width: 10, height: 10 }).unwrap();
    let info = stack.push_view(Rect { x: 100, y: 100, width: 20, height: 20 }).unwrap();
    assert_eq!((info.bounds.width, info.bounds.height), (20, 20));
    assert!(info.clip.width == 0 || info.clip.height == 0);
    assert!(contains(info.viewport, info.clip));
}

/// Root values are already final visible geometry, not the original unbounded allocation.
#[test]
fn invalid_roots_and_negative_extents_reject() {
    for limit in [0, 65, u32::MAX] {
        assert!(ViewStack::new(root(), limit).is_err());
    }
    let mut invalid = root();
    invalid.client.width -= 1;
    assert!(ViewStack::new(invalid, 4).is_err());
    invalid = root();
    invalid.target.x = -1;
    assert!(ViewStack::new(invalid, 4).is_err());
    invalid = root();
    invalid.target.x = i32::MAX;
    assert!(ViewStack::new(invalid, 4).is_err());
    let mut stack = ViewStack::new(root(), 4).unwrap();
    assert!(stack.push_clip(Rect { x: 0, y: 0, width: -1, height: 1 }).is_err());
    assert_eq!(stack.depth(), 1);
}

/// Integer edge conversion preserves historical wide-content and fractional-DPI cases.
#[test]
fn physical_edges_are_exact_and_preserve_wide_intersections() {
    let logical = Size { width: 32, height: 32 };
    let physical = logical;
    assert_eq!(
        physical_rect(Rect { x: 1, y: 0, width: i32::MAX, height: 32 }, logical, physical).unwrap(),
        Some(PixelRect { x: 1, y: 0, width: 31, height: 32 })
    );
    assert_eq!(
        physical_rect(
            Rect { x: 1, y: 1, width: 1, height: 1 },
            Size { width: 3, height: 3 },
            Size { width: 5, height: 5 }
        )
        .unwrap(),
        Some(PixelRect { x: 1, y: 1, width: 3, height: 3 })
    );
    let large = 16_777_217;
    assert_eq!(
        physical_rect(
            Rect { x: large, y: 0, width: 1, height: 1 },
            Size { width: i32::MAX, height: 1 },
            Size { width: i32::MAX, height: 1 }
        )
        .unwrap(),
        Some(PixelRect {
            x: large as u32,
            y: 0,
            width: 1,
            height: 1
        })
    );
}
