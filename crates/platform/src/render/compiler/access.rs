//! Shared decomposition of addressable reads and writes, independent of native storage classes.
use super::ir::{Expr, ExprKind, Literal, Ty, Unary};

/// A local snapshot or a read-only uniform declaration owns an addressable path.
#[derive(Clone, Copy)]
pub(super) enum Root {
    /// Function-local index; the writer distinguishes immutable values from mutable storage.
    Local(usize),
    /// Module resource index; opaque textures cannot form value paths.
    Uniform(usize),
}

/// One projection preserves its result type and the original index expression's evaluation order.
pub(super) enum Step<'a> {
    /// Structure member or single vector component, with a compile-time index.
    Member {
        /// Validated zero-based member/component.
        index: u32,
        /// Type after this projection.
        ty: &'a Ty,
    },
    /// Array, matrix-column or vector-element selection with read-zero/skip-write semantics.
    Index {
        /// Evaluated exactly once, before any generated bounds branch.
        value: &'a Expr,
        /// Authoritative finite element count.
        count: u32,
        /// Type after this projection.
        ty: &'a Ty,
    },
}

/// An entire projection chain can be guarded before loading its leaf, without copying parents.
pub(super) struct Path<'a> {
    /// Declaration owning the memory or immutable value.
    pub root: Root,
    /// Ordered from the root toward the selected leaf.
    pub steps: Vec<Step<'a>>,
}

/// Classifies the complete expression before writers evaluate indices, so a failed
/// decomposition cannot accidentally evaluate helper calls twice during fallback.
pub(super) fn path(expression: &Expr) -> Option<Path<'_>> {
    fn collect<'a>(expression: &'a Expr, steps: &mut Vec<Step<'a>>) -> Option<Root> {
        let root = match &expression.kind {
            ExprKind::Local(i) => return Some(Root::Local(*i)),
            ExprKind::Resource(i) if expression.ty != Ty::Texture => return Some(Root::Uniform(*i)),
            ExprKind::Field(base, index) => {
                let root = collect(base, steps)?;
                steps.push(Step::Member { index: *index as u32, ty: &expression.ty });
                root
            }
            ExprKind::Swizzle(base, lanes) if lanes.len() == 1 => {
                let root = collect(base, steps)?;
                steps.push(Step::Member {
                    index: u32::from(lanes[0]),
                    ty: &expression.ty,
                });
                root
            }
            ExprKind::Index(base, value, count) => {
                let root = collect(base, steps)?;
                steps.push(Step::Index { value, count: *count, ty: &expression.ty });
                root
            }
            _ => return None,
        };
        Some(root)
    }
    let mut steps = Vec::new();
    let root = collect(expression, &mut steps)?;
    Some(Path { root, steps })
}

/// Proven integer constants need no runtime bounds branches. Signed negatives are
/// reinterpreted unsigned, matching the dynamic check; no floating-point or
/// overflowing arithmetic is folded with potentially different backend semantics.
pub(super) fn constant_index(expression: &Expr) -> Option<u32> {
    if !matches!(expression.ty, Ty::Scalar(super::ir::Scalar::Sint | super::ir::Scalar::Uint)) {
        return None;
    }
    match &expression.kind {
        ExprKind::Literal(Literal::Uint(value)) => Some(*value),
        ExprKind::Literal(Literal::Sint(value)) => Some(*value as u32),
        ExprKind::Construct(values)
            if values.len() == 1
                && values[0]
                    .ty
                    .scalar()
                    .is_some_and(|s| matches!(s, super::ir::Scalar::Sint | super::ir::Scalar::Uint)) =>
        {
            constant_index(&values[0])
        }
        ExprKind::Unary(Unary::Negate, value) => (constant_index(value)? as i32).checked_neg().map(|v| v as u32),
        _ => None,
    }
}
