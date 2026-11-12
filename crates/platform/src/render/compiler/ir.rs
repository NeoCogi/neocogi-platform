//! Typed graphics IR. Validation finishes for every declaration before any writer runs.
use super::{
    Diagnostic,
    syntax::{Node, Value},
};
use neocogi_platform_contracts::render::{
    abi::GpuShaderStage,
    shader::{IoLocation, IoShape, ShaderScalar, ShaderInterpolation, ShaderSampling},
};
use std::collections::{BTreeMap, BTreeSet};

/// Scalar families are deliberately limited to portable 32-bit values and booleans.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum Scalar {
    /// IEEE 32-bit floating-point components; source literals must be finite.
    Float,
    /// Signed two-complement 32-bit integers.
    Sint,
    /// Unsigned 32-bit integers.
    Uint,
    /// Boolean values, excluded from uniform storage.
    Bool,
}
/// Concrete, finite value types; structures refer to prior declarations only.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum Ty {
    /// One portable scalar value.
    Scalar(Scalar),
    /// Scalar family and lane count in 2..4.
    Vector(Scalar, u8),
    /// Column count and row count in 2..4, with f32 components.
    Matrix(u8, u8),
    /// Finite element type and fixed length in 1..4096.
    Array(Box<Ty>, u32),
    /// Index of a previously declared nonrecursive structure.
    Struct(usize),
    /// Absence of a helper result; never a storable value.
    Void,
    /// Opaque paired texture resource; never a storable value or function result.
    Texture,
}
impl Ty {
    /// Component family, excluding composites and void.
    pub fn scalar(&self) -> Option<Scalar> {
        match self {
            Self::Scalar(s) | Self::Vector(s, _) => Some(*s),
            Self::Matrix(..) => Some(Scalar::Float),
            _ => None,
        }
    }
    /// Number of scalar/vector lanes; matrices and aggregates are not vectors.
    pub fn lanes(&self) -> u8 {
        match self {
            Self::Vector(_, n) => *n,
            _ => 1,
        }
    }
    /// Boolean shape of componentwise comparisons.
    pub fn boolean(&self) -> Ty {
        if self.lanes() == 1 {
            Ty::Scalar(Scalar::Bool)
        } else {
            Ty::Vector(Scalar::Bool, self.lanes())
        }
    }
}
/// An explicit location or one of the supported graphics builtins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Binding {
    /// Portable location with its explicit normalized interpolation and sampling.
    Location(u32, ShaderInterpolation, ShaderSampling),
    /// Stage/direction/type-checked graphics builtin.
    Builtin(BuiltinIo),
}
/// Stage IO with exact required value types.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuiltinIo {
    /// Vertex clip output or fragment framebuffer-position input, always vec4.
    Position,
    /// Vertex input u32 index, including the native base vertex.
    VertexIndex,
    /// Vertex input u32 instance index.
    InstanceIndex,
    /// Fragment input bool with the renderer's canonical winding.
    FrontFacing,
}
/// Named typed field, parameter, or return value with optional interface binding.
#[derive(Clone, Debug)]
pub(crate) struct Field {
    /// Source member or parameter name; writers use resolved indices instead.
    pub name: String,
    /// Validated finite value type.
    pub ty: Ty,
    /// Optional explicit graphics interface contract.
    pub binding: Option<Binding>,
    /// Source byte offset for binding/type diagnostics.
    pub offset: usize,
}
/// A finite structure with ordered fields; layout is derived from types.
#[derive(Clone, Debug)]
pub(crate) struct Structure {
    /// Unique source type name used during resolution.
    pub name: String,
    /// Ordered members; their order defines construction, offsets, and member indices.
    pub fields: Vec<Field>,
}
/// Read-only, stage-local binding. A texture declaration owns its implicit paired sampler.
#[derive(Clone, Debug)]
pub(crate) struct Resource {
    /// Unique source binding name.
    pub name: String,
    /// Uniform structure type or the opaque paired texture type.
    pub ty: Ty,
    /// The sole stage authorized to read this binding.
    pub stage: GpuShaderStage,
    /// Portable stage-local slot, always below four.
    pub slot: u32,
    /// Declaration offset used for stage and pairing failures.
    pub offset: usize,
}
/// Literal bits avoid host Debug formatting and retain exact scalar type.
#[derive(Clone, Debug)]
pub(crate) enum Literal {
    /// Finite f32 value, emitted with Display or exact binary bits.
    Float(f32),
    /// Exact signed integer bits.
    Sint(i32),
    /// Exact unsigned integer bits.
    Uint(u32),
    /// Explicit true or false, without numeric conversion.
    Bool(bool),
}
/// Unary operators are validated before emission.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Unary {
    /// Sign negation for floating-point or signed-integer scalars/vectors.
    Negate,
    /// Componentwise boolean negation.
    Not,
    /// Componentwise 32-bit integer complement.
    Complement,
}
/// Binary operators with backend-independent semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Binary {
    /// Numeric addition, including compatible matrices and scalar broadcasts.
    Add,
    /// Numeric subtraction, including compatible matrices and scalar broadcasts.
    Subtract,
    /// Componentwise numeric multiplication or dimension-checked matrix product.
    Multiply,
    /// Numeric quotient, with matrix/scalar division lowered by columns.
    Divide,
    /// Numeric remainder; floating-point lowering uses truncation.
    Remainder,
    /// Componentwise equality, producing the corresponding boolean shape.
    Equal,
    /// Componentwise inequality, producing the corresponding boolean shape.
    NotEqual,
    /// Numeric componentwise ordering.
    Less,
    /// Numeric componentwise nonstrict ordering.
    LessEqual,
    /// Numeric componentwise reverse ordering.
    Greater,
    /// Numeric componentwise reverse nonstrict ordering.
    GreaterEqual,
    /// Scalar boolean conjunction with short-circuit evaluation.
    And,
    /// Scalar boolean disjunction with short-circuit evaluation.
    Or,
    /// Componentwise integer bit intersection.
    BitAnd,
    /// Componentwise integer bit union.
    BitOr,
    /// Componentwise integer exclusive bit union.
    Xor,
    /// Integer left shift with a same-shaped unsigned count.
    ShiftLeft,
    /// Arithmetic signed or logical unsigned right shift.
    ShiftRight,
}
/// Portable intrinsics; exhaustive writer matches prevent accidental unsupported emission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Builtin {
    /// Absolute value for signed or floating-point values.
    Abs,
    /// Componentwise numeric minimum.
    Min,
    /// Componentwise numeric maximum.
    Max,
    /// Componentwise numeric lower/upper clamping.
    Clamp,
    /// Round f32 values toward negative infinity.
    Floor,
    /// Round f32 values toward positive infinity.
    Ceil,
    /// Fractional part relative to floor.
    Fract,
    /// Round f32 values to nearest with even ties.
    Round,
    /// Round f32 values toward zero.
    Trunc,
    /// Componentwise floating-point square root.
    Sqrt,
    /// Componentwise reciprocal square root.
    InverseSqrt,
    /// Componentwise sine in radians.
    Sin,
    /// Componentwise cosine in radians.
    Cos,
    /// Componentwise tangent in radians.
    Tan,
    /// Componentwise inverse sine.
    Asin,
    /// Componentwise inverse cosine.
    Acos,
    /// Componentwise inverse tangent.
    Atan,
    /// Two-argument componentwise inverse tangent.
    Atan2,
    /// Componentwise natural exponential.
    Exp,
    /// Componentwise base-two exponential.
    Exp2,
    /// Componentwise natural logarithm.
    Log,
    /// Componentwise base-two logarithm.
    Log2,
    /// Componentwise floating-point power.
    Pow,
    /// Floating-point vector dot product producing f32.
    Dot,
    /// Three-component floating-point vector cross product.
    Cross,
    /// Floating-point vector unit-direction operation.
    Normalize,
    /// Scalar/vector Euclidean length producing f32.
    Length,
    /// Distance between same-shaped floating-point vectors.
    Distance,
    /// Floating-point vector reflection about a supplied normal.
    Reflect,
    /// Componentwise floating-point linear interpolation.
    Mix,
    /// Eager scalar/vector selection controlled by scalar or vector booleans.
    Select,
    /// Canonical fragment X derivative.
    Dpdx,
    /// Canonical signed fragment Y derivative.
    Dpdy,
    /// Sum of absolute X/Y derivatives.
    Fwidth,
    /// Implicit-derivative paired texture sample, requiring uniform fragment control.
    Sample,
    /// Explicit mip-zero paired texture sample.
    SampleLevel,
    /// Guarded integer-coordinate mip-zero texel read.
    Load,
    /// Mip-zero texture width/height as uvec2.
    Dimensions,
    /// Boolean-vector conjunction producing scalar bool.
    All,
    /// Boolean-vector disjunction producing scalar bool.
    Any,
}
/// Fully typed expression. References use resolved numeric indices, never writer lookups.
#[derive(Clone, Debug)]
pub(crate) struct Expr {
    /// Exact result type, established before any writer runs.
    pub ty: Ty,
    /// Resolved operation and owned operand tree.
    pub kind: ExprKind,
}
/// Expressions cannot carry writes or untyped payloads.
#[derive(Clone, Debug)]
pub(crate) enum ExprKind {
    /// Typed scalar constant, with no runtime lookup.
    Literal(Literal),
    /// Resolved index into the current function's locals.
    Local(usize),
    /// Resolved read-only module binding.
    Resource(usize),
    /// Structure value and validated member index.
    Field(Box<Expr>, usize),
    /// Vector value and one to four in-range lane indices.
    Swizzle(Box<Expr>, Vec<u8>),
    /// Aggregate, integer index, and authoritative element count for guards.
    Index(Box<Expr>, Box<Expr>, u32),
    /// Checked unary operation and its operand.
    Unary(Unary, Box<Expr>),
    /// Checked binary operation and ordered operands.
    Binary(Binary, Box<Expr>, Box<Expr>),
    /// Result-type-directed aggregate packing or explicit numeric conversion.
    Construct(Vec<Expr>),
    /// Resolved helper index and ordered value arguments.
    Call(usize, Vec<Expr>),
    /// Validated intrinsic and its ordered operands.
    Builtin(Builtin, Vec<Expr>),
}
/// Lexical local declaration. Parameters occupy the initial indices and are immutable.
#[derive(Clone, Debug)]
pub(crate) struct Local {
    /// Source name used only for lexical resolution.
    pub name: String,
    /// Concrete value type shared by declaration and every use.
    pub ty: Ty,
    /// Only mutable local paths may appear on the left of a store.
    pub mutable: bool,
}
/// Structured statements with validated lvalues and return paths.
#[derive(Clone, Debug)]
pub(crate) enum Stmt {
    /// Local index and optional initializer; absence means zero initialization.
    Let(usize, Option<Expr>),
    /// Validated mutable local path and same-typed value; invalid indices skip the store.
    Store(Expr, Expr),
    /// Scalar condition and two lexical branches.
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    /// Optional result matching the function signature.
    Return(Option<Expr>),
    /// Reevaluated scalar condition and lexical body.
    While(Expr, Vec<Stmt>),
    /// Exit the innermost while loop.
    Break,
    /// Resume the innermost while condition.
    Continue,
}
/// Function with complete transitive resource/call metadata derived during validation.
#[derive(Clone, Debug)]
pub(crate) struct Function {
    /// Unique source helper or entry name.
    pub name: String,
    /// Ordered immutable parameters, occupying the initial local indices.
    pub params: Vec<Field>,
    /// Return type and optional entry binding.
    pub result: Field,
    /// Some only for an externally selectable graphics entry.
    pub stage: Option<GpuShaderStage>,
    /// Parameters and lexically resolved locals with stable function-local indices.
    pub locals: Vec<Local>,
    /// Structured statements with validated types and return paths.
    pub body: Vec<Stmt>,
    /// Transitive module resource indices read by this function.
    pub resources: BTreeSet<usize>,
    /// Direct module function indices, with recursion and entry calls excluded.
    pub calls: BTreeSet<usize>,
    /// Transitive derivatives or implicit samples require fragment execution.
    pub fragment_only: bool,
    /// Transitive texture indices participating in actual sample operations.
    pub sampled: BTreeSet<usize>,
    /// Function declaration offset for control-flow and entry diagnostics.
    pub offset: usize,
}
/// The sole authoritative representation shared by all writers and reflection.
#[derive(Clone, Debug, Default)]
pub(crate) struct Module {
    /// Declaration-ordered, nonrecursive value structures.
    pub structures: Vec<Structure>,
    /// Read-only stage/slot bindings, independent of selected output.
    pub resources: Vec<Resource>,
    /// All validated helpers and entries, including inactive declarations.
    pub functions: Vec<Function>,
}

/// Reject nonportable or colliding source names before backend-specific escaping.
fn name(node: &Node<'_>) -> Result<String, Diagnostic> {
    let value = node.atom()?;
    if value.len() > 128 || value.is_empty() || !value.as_bytes()[0].is_ascii_alphabetic() || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(node.error("identifier must start with a letter and contain at most 128 ASCII letters, digits, or underscores"));
    }
    Ok(value.into())
}
/// Exact arity diagnostics name the syntax form at fault.
fn arity(node: &Node<'_>, values: &[Node<'_>], n: usize) -> Result<(), Diagnostic> {
    if values.len() != n {
        Err(node.error(format!("expected {n} elements, found {}", values.len())))
    } else {
        Ok(())
    }
}
/// Type names have explicit scalar families, with short floating-point vector/matrix spellings.
pub(crate) fn type_name(value: &str) -> Option<Ty> {
    use Scalar::*;
    Some(match value {
        "f32" => Ty::Scalar(Float),
        "i32" => Ty::Scalar(Sint),
        "u32" => Ty::Scalar(Uint),
        "bool" => Ty::Scalar(Bool),
        "void" => Ty::Void,
        _ => {
            let (family, tail) = if let Some(t) = value.strip_prefix("uvec") {
                (Uint, t)
            } else if let Some(t) = value.strip_prefix("ivec") {
                (Sint, t)
            } else if let Some(t) = value.strip_prefix("bvec") {
                (Bool, t)
            } else if let Some(t) = value.strip_prefix("vec") {
                (Float, t)
            } else {
                let t = value.strip_prefix("mat")?;
                let (c, r) = t.split_once('x').unwrap_or((t, t));
                let (c, r): (u8, u8) = (c.parse().ok()?, r.parse().ok()?);
                return ((2..=4).contains(&c) && (2..=4).contains(&r)).then_some(Ty::Matrix(c, r));
            };
            let n = tail.parse::<u8>().ok()?;
            if !(2..=4).contains(&n) {
                return None;
            }
            Ty::Vector(family, n)
        }
    })
}
impl Module {
    /// Resolves a type, with bounded fixed-size arrays and no recursive structures.
    pub fn ty(&self, node: &Node<'_>) -> Result<Ty, Diagnostic> {
        if let Value::Atom(value) = node.value {
            if let Some(ty) = type_name(value) {
                return Ok(ty);
            }
            return self
                .structures
                .iter()
                .position(|s| s.name == value)
                .map(Ty::Struct)
                .ok_or_else(|| node.error(format!("unknown type '{value}'")));
        }
        let list = node.list()?;
        if list.first().and_then(|n| n.atom().ok()) != Some("array") {
            return Err(node.error("expected a type or (array TYPE COUNT)"));
        }
        arity(node, list, 3)?;
        let count = list[2].number()?;
        let base = self.ty(&list[1])?;
        if matches!(base, Ty::Array(..)) {
            return Err(node.error("GLSL ES 3.00 does not support arrays of arrays; use a structure element"));
        }
        if count == 0 || count > 4096 || base == Ty::Void {
            return Err(node.error("array length must be 1..4096 and element type must be a value"));
        }
        let ty = Ty::Array(Box::new(base), count);
        if self.layout(&ty).1 > 65536 {
            return Err(node.error("value type exceeds 64 KiB limit"));
        }
        Ok(ty)
    }
    /// Natural WGSL alignment and byte size. Portable uniforms additionally check std140 strides.
    pub fn layout(&self, ty: &Ty) -> (u32, u32) {
        let round = |n: u32, a: u32| n.div_ceil(a) * a;
        match ty {
            Ty::Scalar(_) => (4, 4),
            Ty::Vector(_, n) => (if *n == 2 { 8 } else { 16 }, u32::from(*n) * 4),
            Ty::Matrix(c, r) => {
                let (a, s) = self.layout(&Ty::Vector(Scalar::Float, *r));
                (a, u32::from(*c) * round(s, a))
            }
            Ty::Array(t, n) => {
                let (a, s) = self.layout(t);
                (a, round(s, a).saturating_mul(*n))
            }
            Ty::Struct(i) => {
                let (mut a, mut s) = (1, 0);
                for f in &self.structures[*i].fields {
                    let (fa, fs) = self.layout(&f.ty);
                    a = a.max(fa);
                    s = round(s, fa).saturating_add(fs);
                }
                (a, round(s, a))
            }
            Ty::Void | Ty::Texture => (1, 0),
        }
    }
    /// Member byte offsets are derived from the same layout used for reflection and SPIR-V.
    pub fn offsets(&self, index: usize) -> Vec<u32> {
        let mut offset = 0u32;
        self.structures[index]
            .fields
            .iter()
            .map(|field| {
                let (a, s) = self.layout(&field.ty);
                offset = offset.div_ceil(a) * a;
                let result = offset;
                offset += s;
                result
            })
            .collect()
    }
    /// Validates identical WGSL/std140 member offsets and matrix/array strides recursively.
    fn uniform_type(&self, ty: &Ty, root: bool) -> bool {
        match ty {
            Ty::Scalar(s) | Ty::Vector(s, _) => *s != Scalar::Bool,
            Ty::Matrix(_, r) => *r >= 3,
            Ty::Array(base, _) => {
                let (a, s) = self.layout(base);
                self.uniform_type(base, false) && (s.div_ceil(a) * a).is_multiple_of(16)
            }
            Ty::Struct(i) => {
                let fields = &self.structures[*i].fields;
                let mut end = 0u32;
                for (field, offset) in fields.iter().zip(self.offsets(*i)) {
                    if !self.uniform_type(&field.ty, false) || field.binding.is_some() {
                        return false;
                    }
                    let (a, s) = self.layout(&field.ty);
                    let a = if matches!(field.ty, Ty::Array(..) | Ty::Struct(..)) { a.max(16) } else { a };
                    end = end.div_ceil(a) * a;
                    if offset != end {
                        return false;
                    }
                    end += s;
                    if matches!(field.ty, Ty::Struct(..)) {
                        end = end.div_ceil(16) * 16;
                    }
                }
                root || self.layout(ty).1.is_multiple_of(16)
            }
            _ => false,
        }
    }
    /// Parses declarations and validates all functions, including inactive entries/helpers.
    pub fn validate(root: &Node<'_>) -> Result<Self, Diagnostic> {
        let mut module = Self::default();
        let mut names = BTreeSet::new();
        let mut bodies = Vec::new();
        let mut slots = BTreeSet::new();
        for node in &root.list()?[1..] {
            let values = node.list()?;
            if values.len() < 2 {
                return Err(node.error("incomplete declaration"));
            }
            let kind = values[0].atom()?;
            let id = name(&values[1])?;
            if !names.insert(id.clone()) || type_name(&id).is_some() || builtin(&id).is_some() {
                return Err(values[1].error("duplicate or reserved declaration name"));
            }
            match kind {
                "struct" => {
                    if values.len() < 3 || values.len() > 66 {
                        return Err(node.error("structures require 1..64 fields"));
                    }
                    let mut fields = Vec::new();
                    for field in &values[2..] {
                        let v = field.list()?;
                        if v.len() < 2 {
                            return Err(field.error("expected (FIELD TYPE [BINDING])"));
                        }
                        let f = module.field(field, &v[0], &v[1], &v[2..])?;
                        if f.ty == Ty::Void || fields.iter().any(|old: &Field| old.name == f.name) {
                            return Err(field.error("duplicate field or void member"));
                        }
                        fields.push(f);
                    }
                    module.structures.push(Structure { name: id, fields });
                    if module.layout(&Ty::Struct(module.structures.len() - 1)).1 > 65536 {
                        return Err(node.error("structure exceeds 64 KiB value limit"));
                    }
                }
                "uniform" | "texture" => {
                    let uniform = kind == "uniform";
                    arity(node, values, if uniform { 5 } else { 4 })?;
                    let ty = if uniform { module.ty(&values[2])? } else { Ty::Texture };
                    let stage = parse_stage(&values[if uniform { 3 } else { 2 }])?;
                    let slot = values[if uniform { 4 } else { 3 }].number()?;
                    if slot >= 4 || !slots.insert((stage.raw(), uniform, slot)) {
                        return Err(node.error("duplicate resource binding or slot outside 0..4"));
                    }
                    if uniform && (!matches!(ty, Ty::Struct(_)) || !module.uniform_type(&ty, true) || module.layout(&ty).1 > 16384) {
                        return Err(node.error("uniform requires a named structure with portable std140 layout and at most 16 KiB"));
                    }
                    module.resources.push(Resource {
                        name: id,
                        ty,
                        stage,
                        slot,
                        offset: node.offset,
                    });
                }
                "fn" | "vertex" | "fragment" => {
                    if values.len() < 5 {
                        return Err(node.error("expected (fn NAME (PARAMETERS) (returns TYPE [BINDING]) BODY...)"));
                    }
                    let mut params = Vec::new();
                    for parameter in values[2].list()? {
                        let p = parameter.list()?;
                        if p.len() < 2 {
                            return Err(parameter.error("expected (PARAMETER TYPE [BINDING])"));
                        }
                        let field = module.field(parameter, &p[0], &p[1], &p[2..])?;
                        if field.ty == Ty::Void || params.iter().any(|f: &Field| f.name == field.name) {
                            return Err(parameter.error("duplicate or void parameter"));
                        }
                        params.push(field);
                    }
                    if params.len() > 16 {
                        return Err(node.error("function parameter limit is sixteen"));
                    }
                    let result = values[3].list()?;
                    if result.len() < 2 || result[0].atom()? != "returns" {
                        return Err(values[3].error("expected (returns TYPE [BINDING])"));
                    }
                    let result = module.field(&values[3], &result[0], &result[1], &result[2..])?;
                    let stage = if kind == "fn" { None } else { Some(parse_stage(&values[0])?) };
                    if stage.is_none() && (result.binding.is_some() || params.iter().any(|field| field.binding.is_some())) {
                        return Err(node.error("helper signatures cannot declare graphics IO bindings"));
                    }
                    module.functions.push(Function {
                        name: id,
                        params,
                        result,
                        stage,
                        locals: Vec::new(),
                        body: Vec::new(),
                        resources: BTreeSet::new(),
                        calls: BTreeSet::new(),
                        fragment_only: false,
                        sampled: BTreeSet::new(),
                        offset: node.offset,
                    });
                    bodies.push(&values[4..]);
                }
                _ => return Err(values[0].error(format!("unsupported declaration '{kind}'"))),
            }
            if module.structures.len() + module.resources.len() + module.functions.len() > 256 {
                return Err(node.error("module declaration limit is 256"));
            }
        }
        for (i, body) in bodies.into_iter().enumerate() {
            let mut checker = Checker::new(&module, i);
            let (statements, returned) = checker.block(body)?;
            if module.functions[i].result.ty != Ty::Void && !returned {
                return Err(Diagnostic {
                    offset: module.functions[i].offset,
                    message: "not every function path returns a value".into(),
                });
            }
            let Checker {
                locals,
                resources,
                calls,
                fragment_only,
                sampled,
                ..
            } = checker;
            let f = &mut module.functions[i];
            f.body = statements;
            f.locals = locals;
            f.resources = resources;
            f.calls = calls;
            f.fragment_only = fragment_only;
            f.sampled = sampled;
        }
        // A bounded DFS rejects recursion and propagates resource/effect metadata once.
        let mut state = vec![0; module.functions.len()];
        for i in 0..module.functions.len() {
            module.resolve_calls(i, &mut state)?;
        }
        for function in &module.functions {
            if !module.uniform_control(&function.body, false).0 {
                return Err(Diagnostic {
                    offset: function.offset,
                    message: "derivatives and implicit samples require unconditional control flow; use sample-level in conditional code".into(),
                });
            }
            if let Some(stage) = function.stage {
                module.interface(function, stage)?;
                if function.fragment_only && stage != GpuShaderStage::Fragment {
                    return Err(Diagnostic {
                        offset: function.offset,
                        message: "derivatives and implicit texture sampling require fragment stage".into(),
                    });
                }
                for &resource in &function.resources {
                    let r = &module.resources[resource];
                    if r.stage != stage {
                        return Err(Diagnostic {
                            offset: r.offset,
                            message: "entry uses a resource from another stage".into(),
                        });
                    }
                    if r.ty == Ty::Texture && !function.sampled.contains(&resource) {
                        return Err(Diagnostic {
                            offset: r.offset,
                            message: "graphics profile requires a paired sampler operation for each used texture".into(),
                        });
                    }
                }
            }
        }
        Ok(module)
    }
    /// Resolves field attributes in one place for members, parameters, and results.
    fn field(&self, node: &Node<'_>, id: &Node<'_>, ty: &Node<'_>, attrs: &[Node<'_>]) -> Result<Field, Diagnostic> {
        let mut binding = None;
        let mut interpolation = ShaderInterpolation::Perspective;
        let mut sampling = ShaderSampling::Center;
        let mut explicit_interpolation = false;
        let ty = self.ty(ty)?;
        for attr in attrs {
            let a = attr.list()?;
            if a.is_empty() {
                return Err(attr.error("empty binding attribute"));
            }
            match a[0].atom()? {
                "location" => {
                    arity(attr, a, 2)?;
                    if binding.is_some() {
                        return Err(attr.error("duplicate IO binding"));
                    }
                    binding = Some(Binding::Location(a[1].number()?, interpolation, sampling));
                }
                "builtin" => {
                    arity(attr, a, 2)?;
                    if binding.is_some() {
                        return Err(attr.error("duplicate IO binding"));
                    }
                    binding = Some(Binding::Builtin(match a[1].atom()? {
                        "position" => BuiltinIo::Position,
                        "vertex_index" => BuiltinIo::VertexIndex,
                        "instance_index" => BuiltinIo::InstanceIndex,
                        "front_facing" => BuiltinIo::FrontFacing,
                        _ => return Err(a[1].error("unsupported builtin")),
                    }));
                }
                "interpolate" => {
                    if explicit_interpolation {
                        return Err(attr.error("duplicate interpolation attribute"));
                    }
                    explicit_interpolation = true;
                    if a.len() < 2 || a.len() > 3 {
                        return Err(attr.error("expected (interpolate MODE [SAMPLING])"));
                    }
                    interpolation = match a[1].atom()? {
                        "perspective" => ShaderInterpolation::Perspective,
                        "linear" => ShaderInterpolation::Linear,
                        "flat" => ShaderInterpolation::Flat,
                        _ => return Err(a[1].error("unsupported interpolation")),
                    };
                    if interpolation == ShaderInterpolation::Flat && a.len() == 3 {
                        return Err(attr.error("flat interpolation cannot carry center or centroid sampling"));
                    }
                    sampling = if a.len() == 3 {
                        match a[2].atom()? {
                            "center" => ShaderSampling::Center,
                            "centroid" => ShaderSampling::Centroid,
                            _ => return Err(a[2].error("sampling must be center or centroid")),
                        }
                    } else if interpolation == ShaderInterpolation::Flat {
                        ShaderSampling::None
                    } else {
                        ShaderSampling::Center
                    };
                }
                _ => return Err(attr.error("unknown IO attribute")),
            }
        }
        if explicit_interpolation && !matches!(binding, Some(Binding::Location(..))) {
            return Err(node.error("interpolation requires a location binding"));
        }
        if explicit_interpolation && ty.scalar() == Some(Scalar::Uint) && interpolation != ShaderInterpolation::Flat {
            return Err(node.error("integer locations require flat interpolation"));
        }
        if let Some(Binding::Location(n, _, _)) = binding {
            binding = Some(Binding::Location(
                n,
                if ty.scalar() == Some(Scalar::Uint) {
                    ShaderInterpolation::Flat
                } else {
                    interpolation
                },
                if ty.scalar() == Some(Scalar::Uint) { ShaderSampling::None } else { sampling },
            ));
        }
        Ok(Field {
            name: name(id)?,
            ty,
            binding,
            offset: node.offset,
        })
    }
    /// Recursion is forbidden; a callee's entry decoration is never callable.
    fn resolve_calls(&mut self, i: usize, state: &mut [u8]) -> Result<(), Diagnostic> {
        if state[i] == 2 {
            return Ok(());
        }
        if state[i] == 1 {
            return Err(Diagnostic {
                offset: self.functions[i].offset,
                message: "recursive function calls are unsupported".into(),
            });
        }
        state[i] = 1;
        let calls = self.functions[i].calls.clone();
        for c in calls {
            if self.functions[c].stage.is_some() {
                return Err(Diagnostic {
                    offset: self.functions[i].offset,
                    message: "entry points cannot be called as helpers".into(),
                });
            }
            self.resolve_calls(c, state)?;
            let resources = self.functions[c].resources.clone();
            let sampled = self.functions[c].sampled.clone();
            let fragment_only = self.functions[c].fragment_only;
            self.functions[i].resources.extend(resources);
            self.functions[i].sampled.extend(sampled);
            self.functions[i].fragment_only |= fragment_only;
        }
        state[i] = 2;
        Ok(())
    }
    /// Tracks early exits conservatively; effectful operations are admitted only in unconditional control flow.
    fn uniform_control(&self, statements: &[Stmt], mut conditional: bool) -> (bool, bool) {
        let mut exits = false;
        for statement in statements {
            match statement {
                Stmt::Let(_, value) => {
                    if value.as_ref().is_some_and(|e| !self.uniform_expression(e, conditional)) {
                        return (false, exits);
                    }
                }
                Stmt::Store(target, value) => {
                    if !self.uniform_expression(target, conditional) || !self.uniform_expression(value, conditional) {
                        return (false, exits);
                    }
                }
                Stmt::Return(value) => {
                    if value.as_ref().is_some_and(|e| !self.uniform_expression(e, conditional)) {
                        return (false, true);
                    }
                    exits = true;
                }
                Stmt::If(condition, yes, no) => {
                    if !self.uniform_expression(condition, conditional) {
                        return (false, exits);
                    }
                    let (valid_yes, exit_yes) = self.uniform_control(yes, true);
                    let (valid_no, exit_no) = self.uniform_control(no, true);
                    if !valid_yes || !valid_no {
                        return (false, exits);
                    }
                    conditional |= exit_yes || exit_no;
                    exits |= exit_yes || exit_no;
                }
                Stmt::While(condition, body) => {
                    let (valid, exit) = self.uniform_control(body, true);
                    if !valid || !self.uniform_expression(condition, true) {
                        return (false, exits);
                    }
                    conditional |= exit;
                    exits |= exit;
                }
                Stmt::Break | Stmt::Continue => {}
            }
        }
        (true, exits)
    }

    /// Checks expressions recursively, including the conditionally evaluated right side of logical operators.
    fn uniform_expression(&self, expression: &Expr, conditional: bool) -> bool {
        match &expression.kind {
            ExprKind::Literal(_) | ExprKind::Local(_) | ExprKind::Resource(_) => true,
            ExprKind::Field(base, _) | ExprKind::Swizzle(base, _) | ExprKind::Unary(_, base) => self.uniform_expression(base, conditional),
            ExprKind::Index(base, index, _) => self.uniform_expression(base, conditional) && self.uniform_expression(index, conditional),
            ExprKind::Binary(op, left, right) => {
                self.uniform_expression(left, conditional) && self.uniform_expression(right, conditional || matches!(op, Binary::And | Binary::Or))
            }
            ExprKind::Construct(args) => args.iter().all(|e| self.uniform_expression(e, conditional)),
            ExprKind::Call(index, args) => {
                (!conditional || !self.functions[*index].fragment_only) && args.iter().all(|e| self.uniform_expression(e, conditional))
            }
            ExprKind::Builtin(op, args) => {
                (!conditional || !matches!(op, Builtin::Dpdx | Builtin::Dpdy | Builtin::Fwidth | Builtin::Sample))
                    && args.iter().all(|e| self.uniform_expression(e, conditional))
            }
        }
    }

    /// Reflects flattened interface locations and enforces builtin direction/type rules.
    pub fn io(&self, fields: &[Field], stage: GpuShaderStage, input: bool) -> Result<(Vec<IoLocation>, bool), Diagnostic> {
        let mut locations = Vec::new();
        let mut builtins = BTreeSet::new();
        let mut position = false;
        let mut pending: Vec<_> = fields.iter().collect();
        while let Some(field) = pending.pop() {
            let error = |m: &str| Diagnostic { offset: field.offset, message: m.into() };
            match &field.binding {
                Some(Binding::Location(location, interpolation, sampling)) => {
                    if *location >= 16
                        || locations.iter().any(|l: &IoLocation| l.location == *location)
                        || !matches!(field.ty, Ty::Scalar(Scalar::Float | Scalar::Uint) | Ty::Vector(Scalar::Float | Scalar::Uint, _))
                    {
                        return Err(error("graphics locations require unique f32/u32 scalar/vector values below sixteen"));
                    }
                    if *interpolation == ShaderInterpolation::Linear {
                        return Err(error("linear interpolation is unavailable in GLSL ES 3.00"));
                    }
                    locations.push(IoLocation {
                        location: *location,
                        shape: IoShape {
                            kind: if field.ty.scalar() == Some(Scalar::Uint) {
                                ShaderScalar::Uint32
                            } else {
                                ShaderScalar::Float32
                            },
                            components: field.ty.lanes(),
                        },
                        interpolation: *interpolation,
                        sampling: *sampling,
                    });
                }
                Some(Binding::Builtin(b)) => {
                    if !builtins.insert(*b as u8) {
                        return Err(error("duplicate builtin"));
                    }
                    let valid = match b {
                        BuiltinIo::Position => {
                            position = true;
                            field.ty == Ty::Vector(Scalar::Float, 4)
                                && ((stage == GpuShaderStage::Vertex && !input) || (stage == GpuShaderStage::Fragment && input))
                        }
                        BuiltinIo::VertexIndex | BuiltinIo::InstanceIndex => stage == GpuShaderStage::Vertex && input && field.ty == Ty::Scalar(Scalar::Uint),
                        BuiltinIo::FrontFacing => stage == GpuShaderStage::Fragment && input && field.ty == Ty::Scalar(Scalar::Bool),
                    };
                    if !valid {
                        return Err(error("builtin has an invalid stage, direction, or type"));
                    }
                }
                None => match field.ty {
                    Ty::Struct(i) => {
                        let members = &self.structures[i].fields;
                        if members.iter().any(|member| member.binding.is_none()) {
                            return Err(error(
                                "entry IO structure members require direct location or builtin bindings; nested IO structures are unsupported",
                            ));
                        }
                        pending.extend(members.iter());
                    }
                    Ty::Void if !input => {}
                    _ => return Err(error("graphics IO requires explicit locations or builtins")),
                },
            }
        }
        locations.sort_by_key(|l| l.location);
        Ok((locations, position))
    }
    /// Enforces vertex position and the single fragment color output across every entry.
    fn interface(&self, f: &Function, stage: GpuShaderStage) -> Result<(), Diagnostic> {
        self.io(&f.params, stage, true)?;
        let (outputs, position) = self.io(std::slice::from_ref(&f.result), stage, false)?;
        let valid = if stage == GpuShaderStage::Vertex {
            position
        } else {
            outputs.len() == 1
                && outputs[0].location == 0
                && outputs[0].shape
                    == IoShape {
                        kind: ShaderScalar::Float32,
                        components: 4,
                    }
                && !position
        };
        if !valid {
            return Err(Diagnostic {
                offset: f.offset,
                message: "vertex requires position; fragment requires exactly one vec4 color at location zero".into(),
            });
        }
        Ok(())
    }
}
/// Only graphics stages are admitted, with no compute/workgroup side channel.
fn parse_stage(node: &Node<'_>) -> Result<GpuShaderStage, Diagnostic> {
    match node.atom()? {
        "vertex" => Ok(GpuShaderStage::Vertex),
        "fragment" => Ok(GpuShaderStage::Fragment),
        _ => Err(node.error("stage must be vertex or fragment")),
    }
}

/// Per-function lexical environment; metadata is moved into the function only on success.
struct Checker<'a> {
    /// Immutable declarations and resolved function signatures.
    module: &'a Module,
    /// Index of the function currently being checked.
    function: usize,
    /// Lexical name maps; no inner binding survives its block.
    scopes: Vec<BTreeMap<String, usize>>,
    /// Stable local declarations accumulated across lexical scopes.
    locals: Vec<Local>,
    /// Direct resources read while validating this body.
    resources: BTreeSet<usize>,
    /// Direct helper calls, resolved to module indices.
    calls: BTreeSet<usize>,
    /// Textures actually sampled by this body, before call propagation.
    sampled: BTreeSet<usize>,
    /// Whether this body directly uses fragment-only operations.
    fragment_only: bool,
    /// Enclosing while depth; break/continue require a positive value.
    loops: usize,
}
impl<'a> Checker<'a> {
    /// Parameters start in the outer immutable scope.
    fn new(module: &'a Module, function: usize) -> Self {
        let f = &module.functions[function];
        let locals: Vec<_> = f
            .params
            .iter()
            .map(|p| Local {
                name: p.name.clone(),
                ty: p.ty.clone(),
                mutable: false,
            })
            .collect();
        let scope = locals.iter().enumerate().map(|(i, l)| (l.name.clone(), i)).collect();
        Self {
            module,
            function,
            scopes: vec![scope],
            locals,
            resources: BTreeSet::new(),
            calls: BTreeSet::new(),
            sampled: BTreeSet::new(),
            fragment_only: false,
            loops: 0,
        }
    }
    /// A lexical block returns whether every live path exits this function.
    fn block(&mut self, nodes: &[Node<'_>]) -> Result<(Vec<Stmt>, bool), Diagnostic> {
        self.scopes.push(BTreeMap::new());
        let mut body = Vec::new();
        let mut returned = false;
        for node in nodes {
            if returned {
                return Err(node.error("unreachable statement after return"));
            }
            let v = node.list()?;
            if v.is_empty() {
                return Err(node.error("empty statement"));
            }
            let statement = match v[0].atom()? {
                "let" | "var" => {
                    let mutable = v[0].atom()? == "var";
                    if (mutable && !(3..=4).contains(&v.len())) || (!mutable && v.len() != 3) {
                        return Err(node.error("expected (let NAME VALUE) or (var NAME TYPE [VALUE])"));
                    }
                    let id = name(&v[1])?;
                    let inferred = mutable && v.len() == 3 && self.module.ty(&v[2]).is_err();
                    let value = if mutable && !inferred {
                        v.get(3).map(|n| self.expr(n)).transpose()?
                    } else {
                        Some(self.expr(&v[2])?)
                    };
                    let ty = if mutable && !inferred {
                        self.module.ty(&v[2])?
                    } else {
                        value.as_ref().unwrap().ty.clone()
                    };
                    if matches!(ty, Ty::Void | Ty::Texture) || value.as_ref().is_some_and(|e| e.ty != ty) {
                        return Err(node.error("local initializer type differs from declared type"));
                    }
                    if self.scopes.last().unwrap().contains_key(&id) || self.module.resources.iter().any(|r| r.name == id) {
                        return Err(v[1].error("duplicate local or resource name"));
                    }
                    if self.locals.len() >= 1024 {
                        return Err(node.error("local variable limit exceeded"));
                    }
                    let index = self.locals.len();
                    self.locals.push(Local { name: id.clone(), ty, mutable });
                    self.scopes.last_mut().unwrap().insert(id, index);
                    Stmt::Let(index, value)
                }
                "set" => {
                    arity(node, v, 3)?;
                    let target = self.expr(&v[1])?;
                    if !self.writable(&target) {
                        return Err(v[1].error("assignment requires a mutable local lvalue"));
                    }
                    let value = self.expr(&v[2])?;
                    if value.ty != target.ty {
                        return Err(v[2].error("assignment type mismatch"));
                    }
                    Stmt::Store(target, value)
                }
                "return" => {
                    if v.len() > 2 {
                        return Err(node.error("return accepts at most one value"));
                    }
                    let value = v.get(1).map(|n| self.expr(n)).transpose()?;
                    if value.as_ref().is_some_and(|e| matches!(e.ty, Ty::Void | Ty::Texture))
                        || value.as_ref().map_or(&Ty::Void, |e| &e.ty) != &self.module.functions[self.function].result.ty
                    {
                        return Err(node.error("return type differs from function result"));
                    }
                    returned = true;
                    Stmt::Return(value)
                }
                "if" => {
                    if v.len() < 3 || v.len() > 4 {
                        return Err(node.error("expected (if CONDITION (then BODY...) [(else BODY...)])"));
                    }
                    let condition = self.condition(&v[1])?;
                    let then = v[2].list()?;
                    if then.first().and_then(|n| n.atom().ok()) != Some("then") {
                        return Err(v[2].error("expected (then ...)"));
                    }
                    let (yes, yr) = self.block(&then[1..])?;
                    let (no, nr) = if let Some(n) = v.get(3) {
                        let values = n.list()?;
                        if values.first().and_then(|n| n.atom().ok()) != Some("else") {
                            return Err(n.error("expected (else ...)"));
                        }
                        self.block(&values[1..])?
                    } else {
                        (Vec::new(), false)
                    };
                    returned = yr && nr;
                    Stmt::If(condition, yes, no)
                }
                "while" => {
                    if v.len() < 3 {
                        return Err(node.error("expected (while CONDITION BODY...)"));
                    }
                    let condition = self.condition(&v[1])?;
                    self.loops += 1;
                    let (body, _) = self.block(&v[2..])?;
                    self.loops -= 1;
                    Stmt::While(condition, body)
                }
                "break" | "continue" => {
                    arity(node, v, 1)?;
                    if self.loops == 0 {
                        return Err(node.error("loop control outside while"));
                    }
                    if v[0].atom()? == "break" { Stmt::Break } else { Stmt::Continue }
                }
                other => return Err(v[0].error(format!("unsupported statement '{other}'"))),
            };
            body.push(statement);
        }
        self.scopes.pop();
        Ok((body, returned))
    }
    /// Conditions are scalar booleans, never implicit numeric conversions.
    fn condition(&mut self, node: &Node<'_>) -> Result<Expr, Diagnostic> {
        let e = self.expr(node)?;
        if e.ty != Ty::Scalar(Scalar::Bool) {
            return Err(node.error("condition must be bool"));
        }
        Ok(e)
    }
    /// Only mutable locals and nonduplicating component/member/index chains can be assigned.
    fn writable(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Local(i) => self.locals[*i].mutable,
            ExprKind::Field(base, _) | ExprKind::Index(base, ..) => self.writable(base),
            ExprKind::Swizzle(base, lanes) => lanes.len() == 1 && self.writable(base),
            _ => false,
        }
    }
    /// Resolves and type-checks expressions; unsupported operators fail here for all targets.
    fn expr(&mut self, node: &Node<'_>) -> Result<Expr, Diagnostic> {
        use Scalar::*;
        if let Value::Atom(value) = node.value {
            let literal = if value == "true" || value == "false" {
                Some(Literal::Bool(value == "true"))
            } else if value.as_bytes().first().is_some_and(|b| b.is_ascii_digit() || matches!(*b, b'-' | b'.' | b'+')) {
                Some(if let Some(v) = value.strip_suffix('u') {
                    Literal::Uint(v.parse().map_err(|_| node.error("invalid u32 literal"))?)
                } else if value.contains(['.', 'e', 'E']) {
                    let v = value.parse::<f32>().map_err(|_| node.error("invalid f32 literal"))?;
                    if !v.is_finite() {
                        return Err(node.error("f32 literal must be finite"));
                    }
                    Literal::Float(v)
                } else {
                    Literal::Sint(value.parse().map_err(|_| node.error("invalid i32 literal"))?)
                })
            } else {
                None
            };
            if let Some(literal) = literal {
                let ty = Ty::Scalar(match literal {
                    Literal::Float(_) => Float,
                    Literal::Sint(_) => Sint,
                    Literal::Uint(_) => Uint,
                    Literal::Bool(_) => Bool,
                });
                return Ok(Expr { ty, kind: ExprKind::Literal(literal) });
            }
            for scope in self.scopes.iter().rev() {
                if let Some(&i) = scope.get(value) {
                    return Ok(Expr {
                        ty: self.locals[i].ty.clone(),
                        kind: ExprKind::Local(i),
                    });
                }
            }
            if let Some(i) = self.module.resources.iter().position(|r| r.name == value) {
                self.resources.insert(i);
                return Ok(Expr {
                    ty: self.module.resources[i].ty.clone(),
                    kind: ExprKind::Resource(i),
                });
            }
            return Err(node.error(format!("unknown value '{value}'")));
        }
        let v = node.list()?;
        if v.is_empty() {
            return Err(node.error("empty expression"));
        }
        let op = v[0].atom().ok();
        if op == Some(".") {
            arity(node, v, 3)?;
            let base = self.expr(&v[1])?;
            let field = v[2].atom()?;
            let (ty, kind) = match base.ty {
                Ty::Struct(i) => {
                    let index = self.module.structures[i]
                        .fields
                        .iter()
                        .position(|f| f.name == field)
                        .ok_or_else(|| v[2].error("unknown structure field"))?;
                    (self.module.structures[i].fields[index].ty.clone(), ExprKind::Field(Box::new(base), index))
                }
                Ty::Vector(s, n) => {
                    let mut lanes = Vec::new();
                    let xyzw = field.bytes().all(|c| b"xyzw".contains(&c));
                    let alphabet = if xyzw { b"xyzw" } else { b"rgba" };
                    for c in field.bytes() {
                        let lane = alphabet.iter().position(|b| *b == c).ok_or_else(|| v[2].error("invalid vector swizzle"))? as u8;
                        if lane >= n {
                            return Err(v[2].error("swizzle component exceeds vector width"));
                        }
                        lanes.push(lane);
                    }
                    if lanes.is_empty() || lanes.len() > 4 {
                        return Err(v[2].error("swizzle must have 1..4 components"));
                    }
                    (
                        if lanes.len() == 1 { Ty::Scalar(s) } else { Ty::Vector(s, lanes.len() as u8) },
                        ExprKind::Swizzle(Box::new(base), lanes),
                    )
                }
                _ => return Err(v[1].error("member access requires a structure or vector")),
            };
            return Ok(Expr { ty, kind });
        }
        if op == Some("index") {
            arity(node, v, 3)?;
            let base = self.expr(&v[1])?;
            let index = self.expr(&v[2])?;
            if !matches!(index.ty, Ty::Scalar(Uint | Sint)) {
                return Err(v[2].error("index must be i32 or u32"));
            }
            let (ty, count) = match &base.ty {
                Ty::Array(t, n) => (t.as_ref().clone(), *n),
                Ty::Vector(s, n) => (Ty::Scalar(*s), u32::from(*n)),
                Ty::Matrix(c, r) => (Ty::Vector(Float, *r), u32::from(*c)),
                _ => return Err(v[1].error("index requires an array, vector, or matrix")),
            };
            return Ok(Expr {
                ty,
                kind: ExprKind::Index(Box::new(base), Box::new(index), count),
            });
        }
        let args = v[1..].iter().map(|n| self.expr(n)).collect::<Result<Vec<_>, _>>()?;
        if matches!(op, Some("-" | "!" | "~")) && args.len() == 1 {
            let arg = args.into_iter().next().unwrap();
            let operator = match op.unwrap() {
                "-" if matches!(arg.ty, Ty::Scalar(Float | Sint) | Ty::Vector(Float | Sint, _)) => Unary::Negate,
                "!" if arg.ty.scalar() == Some(Bool) => Unary::Not,
                "~" if matches!(arg.ty.scalar(), Some(Sint | Uint)) => Unary::Complement,
                _ => return Err(node.error("invalid unary operand type")),
            };
            return Ok(Expr {
                ty: arg.ty.clone(),
                kind: ExprKind::Unary(operator, Box::new(arg)),
            });
        }
        if let Some(operator) = op.and_then(binary) {
            if args.len() != 2 {
                return Err(node.error("binary operator requires two operands"));
            }
            let mut it = args.into_iter();
            let a = it.next().unwrap();
            let b = it.next().unwrap();
            let ty = binary_type(operator, &a.ty, &b.ty).ok_or_else(|| node.error("binary operand types do not match the operator"))?;
            return Ok(Expr {
                ty,
                kind: ExprKind::Binary(operator, Box::new(a), Box::new(b)),
            });
        }
        let construct = if matches!(v[0].value, Value::List(_)) {
            Some(self.module.ty(&v[0])?)
        } else {
            op.and_then(type_name)
                .or_else(|| self.module.structures.iter().position(|s| Some(s.name.as_str()) == op).map(Ty::Struct))
        };
        if let Some(ty) = construct {
            validate_constructor(self.module, &ty, &args).map_err(|m| node.error(m))?;
            return Ok(Expr { ty, kind: ExprKind::Construct(args) });
        }
        if let Some(intrinsic) = op.and_then(builtin) {
            let ty = intrinsic_type(intrinsic, &args).ok_or_else(|| node.error(format!("invalid arguments to '{}'", op.unwrap())))?;
            if matches!(intrinsic, Builtin::Dpdx | Builtin::Dpdy | Builtin::Fwidth | Builtin::Sample) {
                self.fragment_only = true;
            }
            if matches!(intrinsic, Builtin::Sample | Builtin::SampleLevel | Builtin::Load | Builtin::Dimensions) {
                let resource = match args.first().map(|a| &a.kind) {
                    Some(ExprKind::Resource(i)) if self.module.resources[*i].ty == Ty::Texture => *i,
                    _ => return Err(node.error("image operations require a declared texture name")),
                };
                if matches!(intrinsic, Builtin::Sample | Builtin::SampleLevel) {
                    self.sampled.insert(resource);
                }
            }
            return Ok(Expr {
                ty,
                kind: ExprKind::Builtin(intrinsic, args),
            });
        }
        if let Some(i) = self.module.functions.iter().position(|f| Some(f.name.as_str()) == op) {
            let f = &self.module.functions[i];
            if args.len() != f.params.len() || args.iter().zip(&f.params).any(|(a, p)| a.ty != p.ty) {
                return Err(node.error("function argument types or count differ from declaration"));
            }
            if f.result.ty == Ty::Void {
                return Err(node.error("void function cannot be used as a value"));
            }
            self.calls.insert(i);
            return Ok(Expr {
                ty: f.result.ty.clone(),
                kind: ExprKind::Call(i, args),
            });
        }
        Err(node.error(format!("unknown operation '{}'", op.unwrap_or("<list>"))))
    }
}
/// Maps syntax operators once, before backend selection.
fn binary(value: &str) -> Option<Binary> {
    use Binary::*;
    Some(match value {
        "+" => Add,
        "-" => Subtract,
        "*" => Multiply,
        "/" => Divide,
        "%" => Remainder,
        "==" => Equal,
        "!=" => NotEqual,
        "<" => Less,
        "<=" => LessEqual,
        ">" => Greater,
        ">=" => GreaterEqual,
        "&&" => And,
        "||" => Or,
        "&" => BitAnd,
        "|" => BitOr,
        "^" => Xor,
        "<<" => ShiftLeft,
        ">>" => ShiftRight,
        _ => return None,
    })
}
/// Component and matrix arithmetic has one authoritative signature checker.
fn binary_type(op: Binary, a: &Ty, b: &Ty) -> Option<Ty> {
    use Binary::*;
    use Scalar::*;
    if matches!(op, And | Or) {
        return (a == &Ty::Scalar(Bool) && a == b).then(|| a.clone());
    }
    if matches!(op, Equal | NotEqual | Less | LessEqual | Greater | GreaterEqual) {
        return (a == b && matches!(a, Ty::Scalar(_) | Ty::Vector(..)) && (!matches!(a.scalar(), Some(Bool)) || matches!(op, Equal | NotEqual)))
            .then(|| a.boolean());
    }
    if matches!(op, ShiftLeft | ShiftRight) {
        return (matches!(a.scalar(), Some(Sint | Uint)) && b.scalar() == Some(Uint) && a.lanes() == b.lanes()).then(|| a.clone());
    }
    if matches!(op, BitAnd | BitOr | Xor) {
        return (a == b && matches!(a.scalar(), Some(Sint | Uint)) && !matches!(a, Ty::Matrix(..))).then(|| a.clone());
    }
    if !matches!(a.scalar(), Some(Float | Sint | Uint)) || a.scalar() != b.scalar() {
        return None;
    }
    if let (Ty::Matrix(ac, ar), Ty::Matrix(bc, br)) = (a, b) {
        return if op == Multiply && ac == br {
            Some(Ty::Matrix(*bc, *ar))
        } else if a == b && matches!(op, Add | Subtract) {
            Some(a.clone())
        } else {
            None
        };
    }
    if let (Ty::Matrix(c, r), Ty::Vector(Float, n)) = (a, b) {
        return (op == Multiply && c == n).then_some(Ty::Vector(Float, *r));
    }
    if let (Ty::Vector(Float, n), Ty::Matrix(c, r)) = (a, b) {
        return (op == Multiply && n == r).then_some(Ty::Vector(Float, *c));
    }
    if a == b {
        return Some(a.clone());
    }
    if matches!(a, Ty::Matrix(..)) && matches!(b, Ty::Scalar(Float)) {
        return matches!(op, Multiply | Divide).then(|| a.clone());
    }
    if matches!(a, Ty::Scalar(Float)) && matches!(b, Ty::Matrix(..)) {
        return (op == Multiply).then(|| b.clone());
    }
    if matches!(op, Add | Subtract | Multiply | Divide) && matches!(a, Ty::Vector(..)) && matches!(b, Ty::Scalar(_)) {
        return Some(a.clone());
    }
    if matches!(op, Add | Subtract | Multiply | Divide) && matches!(b, Ty::Vector(..)) && matches!(a, Ty::Scalar(_)) {
        return Some(b.clone());
    }
    None
}
/// Constructors include explicit numeric conversions and aggregate initialization.
fn validate_constructor(module: &Module, ty: &Ty, args: &[Expr]) -> Result<(), &'static str> {
    use Ty::*;
    let valid = match ty {
        Scalar(s) => args.len() == 1 && matches!(args[0].ty, Scalar(_)) && ((*s == self::Scalar::Bool) == (args[0].ty.scalar() == Some(self::Scalar::Bool))),
        Vector(s, n) => {
            (args.len() == 1
                && args[0].ty.lanes() == *n
                && args[0].ty.scalar().is_some()
                && !matches!(args[0].ty, Matrix(..))
                && ((*s == self::Scalar::Bool) == (args[0].ty.scalar() == Some(self::Scalar::Bool))))
                || (args.iter().all(|a| matches!(a.ty, Scalar(_) | Vector(..)) && a.ty.scalar() == Some(*s))
                    && (args.iter().map(|a| u32::from(a.ty.lanes())).sum::<u32>() == u32::from(*n) || (args.len() == 1 && args[0].ty.lanes() == 1)))
        }
        Matrix(c, r) => args.len() == usize::from(*c) && args.iter().all(|a| a.ty == Vector(self::Scalar::Float, *r)),
        Array(t, n) => args.len() == *n as usize && args.iter().all(|a| &a.ty == t.as_ref()),
        Struct(i) => args.len() == module.structures[*i].fields.len() && args.iter().zip(&module.structures[*i].fields).all(|(a, f)| a.ty == f.ty),
        Void | Texture => false,
    };
    if valid {
        Ok(())
    } else {
        Err("constructor arguments do not match the value type")
    }
}
/// Intrinsic spellings intentionally follow familiar WGSL names, with paired texture arguments implicit.
fn builtin(value: &str) -> Option<Builtin> {
    use Builtin::*;
    Some(match value {
        "abs" => Abs,
        "min" => Min,
        "max" => Max,
        "clamp" => Clamp,
        "floor" => Floor,
        "ceil" => Ceil,
        "fract" => Fract,
        "round" => Round,
        "trunc" => Trunc,
        "sqrt" => Sqrt,
        "inverseSqrt" => InverseSqrt,
        "sin" => Sin,
        "cos" => Cos,
        "tan" => Tan,
        "asin" => Asin,
        "acos" => Acos,
        "atan" => Atan,
        "atan2" => Atan2,
        "exp" => Exp,
        "exp2" => Exp2,
        "log" => Log,
        "log2" => Log2,
        "pow" => Pow,
        "dot" => Dot,
        "cross" => Cross,
        "normalize" => Normalize,
        "length" => Length,
        "distance" => Distance,
        "reflect" => Reflect,
        "mix" => Mix,
        "select" => Select,
        "dpdx" | "dpdxFine" | "dpdxCoarse" => Dpdx,
        "dpdy" | "dpdyFine" | "dpdyCoarse" => Dpdy,
        "fwidth" | "fwidthFine" | "fwidthCoarse" => Fwidth,
        "sample" => Sample,
        "sample-level" => SampleLevel,
        "load" => Load,
        "dimensions" => Dimensions,
        "all" => All,
        "any" => Any,
        _ => return None,
    })
}
/// Checks overloads without allocating backend state or accepting arbitrary native builtins.
fn intrinsic_type(op: Builtin, a: &[Expr]) -> Option<Ty> {
    use Builtin::*;
    use Scalar::*;
    let first = a.first()?;
    let t = &first.ty;
    let float = matches!(t, Ty::Scalar(Float) | Ty::Vector(Float, _));
    let numeric = matches!(t, Ty::Scalar(Float | Sint | Uint) | Ty::Vector(Float | Sint | Uint, _));
    let same = |n| a.len() == n && a.iter().all(|e| &e.ty == t);
    let valid = match op {
        Abs => a.len() == 1 && numeric && t.scalar() != Some(Uint),
        Min | Max => same(2) && numeric,
        Clamp => same(3) && numeric,
        Floor | Ceil | Fract | Round | Trunc | Sqrt | InverseSqrt | Sin | Cos | Tan | Asin | Acos | Atan | Exp | Exp2 | Log | Log2 | Dpdx | Dpdy | Fwidth => {
            a.len() == 1 && float
        }
        Atan2 | Pow => same(2) && float,
        Normalize => a.len() == 1 && matches!(t, Ty::Vector(Float, _)),
        Dot | Distance => same(2) && matches!(t, Ty::Vector(Float, _)),
        Cross => same(2) && t == &Ty::Vector(Float, 3),
        Length => a.len() == 1 && float,
        Reflect => same(2) && matches!(t, Ty::Vector(Float, _)),
        Mix => same(3) && float,
        Select => a.len() == 3 && a[1].ty == *t && matches!(t, Ty::Scalar(_) | Ty::Vector(..)) && (a[2].ty == Ty::Scalar(Bool) || a[2].ty == t.boolean()),
        All | Any => a.len() == 1 && matches!(t, Ty::Vector(Bool, _)),
        Sample => a.len() == 2 && *t == Ty::Texture && a[1].ty == Ty::Vector(Float, 2),
        SampleLevel => {
            a.len() == 3 && *t == Ty::Texture && a[1].ty == Ty::Vector(Float, 2) && matches!(a[2].kind,ExprKind::Literal(Literal::Float(v)) if v==0.0)
        }
        Load => a.len() == 2 && *t == Ty::Texture && matches!(a[1].ty, Ty::Vector(Sint | Uint, 2)),
        Dimensions => a.len() == 1 && *t == Ty::Texture,
    };
    if !valid {
        return None;
    }
    Some(match op {
        Dot | Distance | Length => Ty::Scalar(Float),
        All | Any => Ty::Scalar(Bool),
        Sample | SampleLevel | Load => Ty::Vector(Float, 4),
        Dimensions => Ty::Vector(Uint, 2),
        _ => t.clone(),
    })
}
