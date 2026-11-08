//! Portable single-mip image operations and GL adaptation of already guarded SPIR-V.
use naga::{Expression, ImageQuery, Literal, Module};
#[cfg(feature = "shader-glsl")]
use naga::{Arena, Block, Statement};

/// Rejects unsupported image operations throughout original WGSL, including unused functions.
/// Exact loads and dimension queries may use only statically known mip zero.
pub(super) fn validate(module: &Module) -> Result<(), String> {
    for function in module
        .functions
        .iter()
        .map(|(_, function)| function)
        .chain(module.entry_points.iter().map(|entry| &entry.function))
    {
        for (_, expression) in function.expressions.iter() {
            match *expression {
                Expression::ImageLoad { level: Some(level), .. }
                | Expression::ImageQuery {
                    query: ImageQuery::Size { level: Some(level) },
                    ..
                } => {
                    if !zero(&function.expressions[level], module) {
                        return Err("portable image loads and dimension queries require constant mip level 0".into());
                    }
                }
                Expression::ImageQuery {
                    query: ImageQuery::NumLevels | ImageQuery::NumLayers | ImageQuery::NumSamples,
                    ..
                } => return Err("portable single-mip 2D images do not support mip-count, layer-count or sample-count queries".into()),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Recognizes literal/constant integer zero after Naga's canonical constant evaluation.
fn zero(expression: &Expression, module: &Module) -> bool {
    match *expression {
        Expression::Literal(Literal::I32(0) | Literal::U32(0)) => true,
        Expression::Constant(handle) => zero(&module.global_expressions[module.constants[handle].init], module),
        Expression::ZeroValue(ty) => matches!(
            module.types[ty].inner,
            naga::TypeInner::Scalar(naga::Scalar {
                kind: naga::ScalarKind::Sint | naga::ScalarKind::Uint,
                width: 4
            })
        ),
        _ => false,
    }
}

/// Specializes compiler-generated mip bounds queries to the binding contract's one level.
///
/// The original WGSL rejects explicit count queries before emission. SPIR-V still
/// contains full coordinate/level guards for Vulkan. Reimport preserves those
/// branches for GL, but ES300 has no textureQueryLevels. Its known answer is one
/// because every admitted sampled allocation has exactly one mip. Image loads
/// must not be guarded a second time by the GLSL writer after this transformation.
#[cfg(feature = "shader-glsl")]
pub(super) fn specialize_gl(module: &mut Module) {
    for function in module
        .functions
        .iter_mut()
        .map(|(_, function)| function)
        .chain(module.entry_points.iter_mut().map(|entry| &mut entry.function))
    {
        for (_, expression) in function.expressions.iter_mut() {
            if matches!(expression, Expression::ImageQuery { query: ImageQuery::NumLevels, .. }) {
                *expression = Expression::Literal(Literal::U32(1));
            }
        }
        filter_emits(&mut function.body, &function.expressions);
    }
}

/// Removes newly constant expressions from Emit ranges without changing statement scope.
/// Naga literals are already in scope; explicitly emitting one would invalidate IR.
#[cfg(feature = "shader-glsl")]
fn filter_emits(block: &mut Block, expressions: &Arena<Expression>) {
    for (mut statement, span) in std::mem::take(block).span_into_iter() {
        match &mut statement {
            Statement::Emit(range) => {
                let mut run = None;
                for handle in range.clone() {
                    if expressions[handle].needs_pre_emit() {
                        if let Some((first, last)) = run.take() {
                            block.push(Statement::Emit(naga::Range::new_from_bounds(first, last)), span);
                        }
                    } else if let Some((_, ref mut last)) = run {
                        *last = handle;
                    } else {
                        run = Some((handle, handle));
                    }
                }
                if let Some((first, last)) = run {
                    block.push(Statement::Emit(naga::Range::new_from_bounds(first, last)), span);
                }
                continue;
            }
            Statement::Block(child) => filter_emits(child, expressions),
            Statement::If { accept, reject, .. } => {
                filter_emits(accept, expressions);
                filter_emits(reject, expressions);
            }
            Statement::Switch { cases, .. } => {
                for case in cases {
                    filter_emits(&mut case.body, expressions);
                }
            }
            Statement::Loop { body, continuing, .. } => {
                filter_emits(body, expressions);
                filter_emits(continuing, expressions);
            }
            _ => {}
        }
        block.push(statement, span);
    }
}
