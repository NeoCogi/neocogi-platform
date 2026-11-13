//! Python frontend workloads rendered by every native backend and actual WebGL2.
use super::{Case, COLORS, STRIPE};

/// Identical uniform layout to the S-expression corpus, including a large matrix palette.
const DECLARATIONS: &str = r#"class Record:
    matrix: mat4
    color: vec4

class Data:
    palette: array[mat4, 64]
    colors: array[vec4, 4]
    records: array[Record, 4]

data: uniform[Data, fragment, 0]

@vertex
def vs(i: builtin[u32, vertex_index]) -> builtin[vec4, position]:
    positions = [vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0)]
    return vec4(positions[i], 0.5, 1.0)
"#;
/// Complete modules use ordinary Python-like assignment; mutability is inferred by the frontend.
pub(super) fn cases() -> Vec<Case> {
    let definitions = [
        (
            "python_index_snapshot",
            "",
            r#"    i = i32(p.x) - 1
    record = data.records[i]
    i += 1
    return record.color
"#,
            STRIPE,
            true,
        ),
        (
            "python_mutable_snapshot",
            "",
            r#"    values = data.colors
    saved = values
    values[0] = vec4(1.0, 1.0, 0.0, 1.0)
    return saved[i32(p.x) - 1]
"#,
            STRIPE,
            true,
        ),
        (
            "python_parameter_snapshot",
            r#"def paint(values: array[vec4, 4], i: i32) -> vec4:
    values[0] = vec4(1.0, 1.0, 0.0, 1.0)
    i -= 1
    return values[i]
"#,
            r#"    values = data.colors
    painted = paint(values, i32(p.x))
    return select(values[i32(p.x) - 1], painted, i32(p.x) == 1)
"#,
            [[0; 4], [255, 255, 0, 255], COLORS[1], COLORS[2], COLORS[3], [0; 4], [0; 4], [0; 4]],
            true,
        ),
        (
            "python_helper_loop",
            r#"def before(values: array[mat4, 64], i: i32) -> bool:
    if i < 0:
        return False
    elif i >= 4:
        return False
    return values[i][0].x >= 0.0
"#,
            r#"    i = 0
    total = 0.0
    while i < i32(p.x) and before(data.palette, i):
        i += 1
        if i == 1:
            continue
        total += 0.25
    return vec4(total, 0.0, 0.0, 1.0)
"#,
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
            "python_augmented_index",
            r#"def index(values: array[vec4, 4], i: i32) -> i32:
    return i - 1
"#,
            r#"    values = data.colors
    values[index(data.colors, i32(p.x))].y += 0.25
    return values[0]
"#,
            [COLORS[0], [255, 64, 0, 255], COLORS[0], COLORS[0], COLORS[0], COLORS[0], COLORS[0], COLORS[0]],
            true,
        ),
        (
            "python_precedence",
            "",
            r#"    amount = -2.0 ** 2.0 + 2.0 ** 3.0 ** 1.0
    amount *= 0.0625
    transform = mat4(
        vec4(1.0, 0.0, 0.0, 0.0),
        vec4(0.0, 1.0, 0.0, 0.0),
        vec4(0.0, 0.0, 1.0, 0.0),
        vec4(0.0, 0.0, 0.0, 1.0),
    )
    value = transform @ vec4(amount, 0.0, 0.0, 1.0)
    flag = not 1.0 > 2.0 and False or True
    return select(vec4(0.0), value, flag)
"#,
            [[64, 0, 0, 255]; 8],
            false,
        ),
        (
            "python_type_name_shadow",
            r#"class Value:
    color: vec4

def modify(Value: Value) -> vec4:
    local = Value
    local.color += vec4(0.25, 0.0, 0.0, 0.0)
    Value.color += vec4(0.5, 0.0, 0.0, 0.0)
    return local.color
"#,
            r#"    value = Value(vec4(0.25, 0.0, 0.0, 1.0))
    result = modify(value)
    return result + (value.color - vec4(0.25, 0.0, 0.0, 1.0))
"#,
            [[128, 0, 0, 255]; 8],
            false,
        ),
    ];
    definitions
        .into_iter()
        .map(|(name, helpers, body, row, uniform)| Case {
            name,
            source: format!("{DECLARATIONS}\n{helpers}\n@fragment\ndef fs(p: builtin[vec4, position]) -> location[vec4, 0]:\n{body}"),
            row,
            uniform,
        })
        .collect()
}
