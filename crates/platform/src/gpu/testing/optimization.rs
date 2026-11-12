//! Deterministic aggregate workloads for code-generation budgets and real native pixels.
use crate::gpu::{
    GpuDevice,
    abi::*,
    native::NativeResources,
    source::{Bytes, PipelineDesc},
};
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset};

/// Uniform layout: 64 matrices, four colors, then four matrix/color records.
pub const UNIFORM_BYTES: u32 = 64 * 64 + 4 * 16 + 4 * 80;
/// Full-screen geometry is independent of the aggregate operation being tested.
pub const VERTEX: &str = r#"(vertex vs ((i u32 (builtin vertex_index))) (returns vec4 (builtin position))
  (let positions ((array vec2 3) (vec2 -1.0 -1.0) (vec2 3.0 -1.0) (vec2 -1.0 3.0)))
  (return (vec4 (index positions i) 0.5 1.0)))"#;
/// Source declarations also exercise arrays nested through structures, without arrays of arrays.
const TYPES: &str = r#"(struct Record (matrix mat4) (color vec4))
  (struct Data (palette (array mat4 64)) (colors (array vec4 4)) (records (array Record 4)))
  (uniform data Data fragment 0)"#;
/// Exactly representable colors make native conversion checks independent of float optimizations.
const COLORS: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255; 4]];
/// No valid value is selected outside the four-element color bank.
const STRIPE: [[u8; 4]; 8] = [[0; 4], COLORS[0], COLORS[1], COLORS[2], COLORS[3], [0; 4], [0; 4], [0; 4]];

/// One source module, its expected physical-pixel row, and its required binding contract.
pub struct Case {
    /// Stable diagnostic name, also suitable for artifact filenames.
    pub name: &'static str,
    /// Actual frontend source containing complete graphics entries and helper declarations.
    pub source: String,
    /// Every row of an 8x8 RGBA8 rendering must match these columns within one unit.
    pub row: [[u8; 4]; 8],
    /// False for pure-expression controls that read no uniform binding.
    pub uniform: bool,
}

/// General access paths, temporal snapshots, branch/loop returns, eager operators and lvalues.
pub fn cases() -> Vec<Case> {
    let definitions = [
        (
            "uniform_nested_fields",
            "",
            r#"(let i (- (i32 (. p x)) 1))
              (let record (index (. data records) i))
              (return (. record color))"#,
            STRIPE,
            true,
        ),
        (
            "uniform_index_snapshot",
            "",
            r#"(var i i32 (- (i32 (. p x)) 1))
              (let record (index (. data records) i))
              (set i (+ i 1))
              (return (. record color))"#,
            STRIPE,
            true,
        ),
        (
            "uniform_matrix_columns",
            "",
            r#"(let i (- (i32 (. p x)) 1))
              (return (index (index (. data palette) i) (- i 1)))"#,
            [
                [0; 4],
                [0; 4],
                COLORS[1],
                [64, 128, 191, 255],
                [191, 64, 128, 255],
                [255, 255, 0, 255],
                [0; 4],
                [0; 4],
            ],
            true,
        ),
        (
            "private_snapshot_alias",
            "",
            r#"(var values (array vec4 4) (. data colors))
              (let saved values)
              (let alias saved)
              (set (index values 0) (vec4 1.0 1.0 0.0 1.0))
              (return (index alias (- (i32 (. p x)) 1)))"#,
            STRIPE,
            true,
        ),
        (
            "returned_snapshot",
            "(fn identity ((values (array vec4 4))) (returns (array vec4 4)) (return values))",
            r#"(var values (array vec4 4) (. data colors))
              (let saved (identity values))
              (set (index values 0) (vec4 1.0 1.0 0.0 1.0))
              (return (index saved (- (i32 (. p x)) 1)))"#,
            STRIPE,
            true,
        ),
        (
            "helper_array_branch",
            r#"(fn pick ((values (array mat4 64)) (i i32)) (returns vec4)
                (if (< i 0) (then (return (vec4 0.0))))
                (let alias values)
                (return (index (index alias i) 0)))"#,
            r#"(let x (i32 (. p x)))
              (let i (select (- x 2) (+ x 60) (>= x 4)))
              (return (pick (. data palette) i))"#,
            [[0; 4], [0; 4], COLORS[0], COLORS[1], [0; 4], [0; 4], [0; 4], [0; 4]],
            true,
        ),
        (
            "helper_array_loop",
            r#"(fn pick ((values (array mat4 64)) (i i32)) (returns vec4)
                (var j i32 0)
                (while (< j 2)
                  (if (== j 1) (then (return (index (index values i) 0))))
                  (set j (+ j 1))
                  (continue))
                (return (vec4 0.0)))"#,
            r#"(let x (i32 (. p x)))
              (let i (select (- x 2) (+ x 60) (>= x 4)))
              (return (pick (. data palette) i))"#,
            [[0; 4], [0; 4], COLORS[0], COLORS[1], [0; 4], [0; 4], [0; 4], [0; 4]],
            true,
        ),
        (
            "helper_in_store_index",
            "(fn first ((values (array mat4 64))) (returns i32) (return 0))",
            r#"(var values (array vec4 4) (. data colors))
              (set (index values (first (. data palette))) (vec4 1.0 1.0 0.0 1.0))
              (return (index values (- (i32 (. p x)) 1)))"#,
            [[0; 4], [255, 255, 0, 255], COLORS[1], COLORS[2], COLORS[3], [0; 4], [0; 4], [0; 4]],
            true,
        ),
        (
            "helper_short_circuit",
            "(fn positive ((values (array mat4 64)) (i i32)) (returns bool) (return (> (. (index (index values i) 0) x) 0.5)))",
            r#"(let x (i32 (. p x)))
              (let value (|| (== x 0) (positive (. data palette) (- x 1))))
              (return (vec4 (select 0.0 1.0 value) 0.0 0.0 1.0))"#,
            [
                [255, 0, 0, 255],
                COLORS[0],
                [0, 0, 0, 255],
                [0, 0, 0, 255],
                COLORS[0],
                COLORS[0],
                [0, 0, 0, 255],
                [0, 0, 0, 255],
            ],
            true,
        ),
        (
            "helper_loop_condition",
            r#"(fn before ((values (array mat4 64)) (i i32)) (returns bool)
                (if (>= i 4) (then (return false)))
                (return (>= (. (index (index values i) 0) x) 0.0)))"#,
            r#"(var i i32 0)
              (var result f32 0.0)
              (while (&& (< i (i32 (. p x))) (before (. data palette) i))
                (set i (+ i 1))
                (if (== i 1) (then (continue)))
                (set result (+ result 0.25)))
              (return (vec4 result 0.0 0.0 1.0))"#,
            [
                [0, 0, 0, 255],
                [0, 0, 0, 255],
                [64, 0, 0, 255],
                [128, 0, 0, 255],
                [191, 0, 0, 255],
                [191, 0, 0, 255],
                [191, 0, 0, 255],
                [191, 0, 0, 255],
            ],
            true,
        ),
        (
            "helper_nested_return",
            r#"(fn leaf ((values (array vec4 4)) (i i32)) (returns vec4)
                (return (index values i)))
              (fn pick ((values (array vec4 4)) (i i32)) (returns vec4)
                (var outer i32 0)
                (while (< outer 2)
                  (var inner i32 0)
                  (while (< inner 2)
                    (set inner (+ inner 1))
                    (if (== inner 1) (then (continue)))
                    (if (== outer 1) (then (return (leaf values i)))))
                  (set outer (+ outer 1)))
                (return (vec4 1.0 0.0 1.0 1.0)))"#,
            r#"(return (pick (. data colors) (- (i32 (. p x)) 1)))"#,
            STRIPE,
            true,
        ),
        (
            "computed_vector_index",
            "(fn value () (returns vec4) (return (vec4 0.25 0.5 0.75 1.0)))",
            r#"(let i (- (i32 (. p x)) 1))
              (return (vec4 (index (value) i) 0.0 0.0 1.0))"#,
            [
                [0, 0, 0, 255],
                [64, 0, 0, 255],
                [128, 0, 0, 255],
                [191, 0, 0, 255],
                COLORS[0],
                [0, 0, 0, 255],
                [0, 0, 0, 255],
                [0, 0, 0, 255],
            ],
            false,
        ),
    ];
    definitions
        .into_iter()
        .map(|(name, helpers, body, row, uniform)| Case {
            name,
            source: format!("(module {TYPES} {helpers} {VERTEX} (fragment fs ((p vec4 (builtin position))) (returns vec4 (location 0)) {body}))"),
            row,
            uniform,
        })
        .collect()
}

/// Explicit bytes honor the same column-major and std140 layout as the frontend reflection.
pub fn uniform_bytes() -> Vec<u8> {
    let mut values = Vec::new();
    for color in COLORS.into_iter().cycle().take(64) {
        values.extend(color.map(|c| f32::from(c) / 255.0));
        values.extend([0.25, 0.5, 0.75, 1.0, 0.75, 0.25, 0.5, 1.0, 1.0, 1.0, 0.0, 1.0]);
    }
    for color in COLORS {
        values.extend(color.map(|c| f32::from(c) / 255.0));
    }
    for color in COLORS {
        values.extend([0.0; 16]);
        values.extend(color.map(|c| f32::from(c) / 255.0));
    }
    let bytes: Vec<_> = values.into_iter().flat_map(f32::to_le_bytes).collect();
    assert_eq!(bytes.len(), UNIFORM_BYTES as usize);
    bytes
}

/// Production COM compilation, binding and recording; no native test shader bypasses the compiler.
pub fn block<B: NativeResources>(gpu: &GpuDevice<B>, resources: &ComPtr<IRenderResourcesAbi>, case: &Case) -> ComBox<IRenderViewBlockAbi> {
    let source = Bytes::source(case.source.as_bytes().to_vec());
    let vertex = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fragment = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let desc = PipelineDesc {
        bindings: if case.uniform {
            vec![GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::Uniform(UNIFORM_BYTES),
            }]
        } else {
            Vec::new()
        },
        ..Default::default()
    }
    .source()
    .unwrap();
    let pipeline = resources
        .create_pipeline(desc.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref())
        .unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut frame = gpu.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    let _uniform = if case.uniform {
        Some(super::bind_uniform(resources.as_com_ref(), &mut view, GpuShaderStage::Fragment, 0, &uniform_bytes()).unwrap())
    } else {
        None
    };
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}

/// One native readback verifies every pixel, including invalid-index alpha and snapshot isolation.
pub fn assert_pixels(case: &Case, pixels: &[u8]) {
    assert_target_pixels(case, pixels, false);
}
/// SDL's opaque default framebuffer has no stored alpha channel; RGB semantics
/// remain identical, and strict RGBA is checked separately on texture targets.
pub fn assert_opaque_pixels(case: &Case, pixels: &[u8]) {
    assert_target_pixels(case, pixels, true);
}
/// Compare the actual target format without treating opaque presentation alpha as shader output.
fn assert_target_pixels(case: &Case, pixels: &[u8], opaque: bool) {
    assert_eq!(pixels.len(), 8 * 8 * 4);
    for row in pixels.as_chunks::<32>().0 {
        for (column, pixel) in row.as_chunks::<4>().0.iter().enumerate() {
            for (channel, (actual, expected)) in pixel.iter().zip(case.row[column]).enumerate() {
                let expected = if opaque && channel == 3 { 255 } else { expected };
                assert!(
                    (i16::from(*actual) - i16::from(expected)).abs() <= 1,
                    "{} column {column}: {pixel:?}, expected {:?}",
                    case.name,
                    case.row[column]
                );
            }
        }
    }
}
