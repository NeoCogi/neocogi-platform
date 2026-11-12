//! WGSL and GLSL ES 3.00 writers sharing structural traversal, not native source parsing.
use super::ir::*;
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl"))]
use super::CompileError;
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl"))]
use neocogi_platform_contracts::render::{
    abi::GpuShaderStage,
    shader::{ShaderInterpolation, ShaderSampling},
};
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl", feature = "shader-spirv"))]
use std::collections::BTreeSet;
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl"))]
use std::fmt::Write;

/// Stage-distinct block names are part of the compiler's native reflection contract.
pub(crate) fn block_name(r: &Resource) -> String {
    format!("nc_block_{}_{}", r.stage.raw(), r.slot)
}
/// Texture and implicit sampler become one native ES300 sampler.
pub(crate) fn texture_name(r: &Resource) -> String {
    format!("nc_texture_{}_{}", r.stage.raw(), r.slot)
}
/// Function closure in callee-before-caller order, independent of authored declaration order.
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl", feature = "shader-spirv"))]
pub(crate) fn reachable(m: &Module, index: usize) -> Vec<usize> {
    fn visit(m: &Module, i: usize, seen: &mut BTreeSet<usize>, out: &mut Vec<usize>) {
        if seen.insert(i) {
            for &c in &m.functions[i].calls {
                visit(m, c, seen, out);
            }
            out.push(i);
        }
    }
    let mut out = Vec::new();
    visit(m, index, &mut BTreeSet::new(), &mut out);
    out
}
/// Bounded output owns the first failure; formatting never silently truncates successful code.
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl"))]
struct Output {
    /// Complete initialized output prefix; never exported after failure.
    text: String,
    /// Maximum code bytes after subtracting the selected entry name.
    limit: usize,
    /// First quota/allocation error is retained across formatting calls.
    error: Option<CompileError>,
}
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl"))]
impl Write for Output {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        if self.error.is_some() {
            return Err(std::fmt::Error);
        }
        let Some(size) = self.text.len().checked_add(s.len()).filter(|n| *n <= self.limit) else {
            self.error = Some(CompileError::Limit);
            return Err(std::fmt::Error);
        };
        if size > self.text.capacity() {
            let capacity = size.max(self.text.capacity().saturating_mul(2)).min(self.limit);
            if self.text.try_reserve_exact(capacity - self.text.len()).is_err() {
                self.error = Some(CompileError::Allocation);
                return Err(std::fmt::Error);
            }
        }
        self.text.push_str(s);
        Ok(())
    }
}
/// One selected writer with per-function temporary numbering.
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl"))]
struct Writer<'a> {
    /// One validated module; no text-language reparsing occurs.
    m: &'a Module,
    /// Selects the ES300 spelling and wrapper; false emits WGSL.
    gl: bool,
    /// Bounded output and its first failure.
    out: Output,
    /// Per-function counter for generated guarded-expression temporaries.
    temp: usize,
    /// Current function index for stable local and type lookup.
    function: usize,
}
/// Emits one complete bounded text artifact.
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl"))]
pub(crate) fn emit(m: &Module, index: usize, gl: bool, limit: usize) -> Result<String, CompileError> {
    let mut w = Writer {
        m,
        gl,
        out: Output { text: String::new(), limit, error: None },
        temp: 0,
        function: index,
    };
    w.module(index);
    if let Some(error) = w.out.error { Err(error) } else { Ok(w.out.text) }
}
#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl"))]
impl Writer<'_> {
    /// Scalar names use Display-based numeric formatting exclusively, including min-size builds.
    fn scalar(&self, s: Scalar) -> &'static str {
        match (self.gl, s) {
            (true, Scalar::Float) => "float",
            (true, Scalar::Sint) => "int",
            (true, Scalar::Uint) => "uint",
            (_, Scalar::Bool) => "bool",
            (_, Scalar::Float) => "f32",
            (_, Scalar::Sint) => "i32",
            (_, Scalar::Uint) => "u32",
        }
    }
    /// Type spelling in constructor position; declarations handle GLSL array suffixes separately.
    fn ty(&self, t: &Ty) -> String {
        match t {
            Ty::Scalar(s) => self.scalar(*s).into(),
            Ty::Vector(s, n) => {
                if self.gl {
                    format!(
                        "{}{n}",
                        match s {
                            Scalar::Float => "vec",
                            Scalar::Sint => "ivec",
                            Scalar::Uint => "uvec",
                            Scalar::Bool => "bvec",
                        }
                    )
                } else {
                    format!("vec{n}<{}>", self.scalar(*s))
                }
            }
            Ty::Matrix(c, r) => {
                if self.gl {
                    format!("mat{c}x{r}")
                } else {
                    format!("mat{c}x{r}<f32>")
                }
            }
            Ty::Array(t, n) => {
                if self.gl {
                    format!("{}[{n}]", self.ty(t))
                } else {
                    format!("array<{}, {n}>", self.ty(t))
                }
            }
            Ty::Struct(i) => format!("nc_s{i}"),
            Ty::Void => "void".into(),
            Ty::Texture => unreachable!("opaque image type cannot be emitted as a value"),
        }
    }
    /// Variable/member declarations avoid invalid GLSL `T[N] name` syntax.
    fn decl(&self, t: &Ty, name: &str) -> String {
        if !self.gl {
            return format!("{name}: {}", self.ty(t));
        }
        match t {
            Ty::Array(base, n) => self.decl(base, &format!("{name}[{n}]")),
            _ => format!("{} {name}", self.ty(t)),
        }
    }
    /// Explicit zero values initialize all private state and out-of-range reads.
    fn zero(&self, t: &Ty) -> String {
        if !self.gl {
            return format!("{}()", self.ty(t));
        }
        match t {
            Ty::Scalar(Scalar::Bool) => "false".into(),
            Ty::Scalar(Scalar::Uint) => "0u".into(),
            Ty::Scalar(Scalar::Sint) => "0".into(),
            Ty::Scalar(Scalar::Float) => "0.0".into(),
            Ty::Vector(s, _) => format!("{}({})", self.ty(t), self.zero(&Ty::Scalar(*s))),
            Ty::Matrix(..) => format!("{}(0.0)", self.ty(t)),
            Ty::Array(base, n) => format!("{}({})", self.ty(t), (0..*n).map(|_| self.zero(base)).collect::<Vec<_>>().join(",")),
            Ty::Struct(i) => format!(
                "{}({})",
                self.ty(t),
                self.m.structures[*i].fields.iter().map(|f| self.zero(&f.ty)).collect::<Vec<_>>().join(",")
            ),
            Ty::Void => String::new(),
            Ty::Texture => unreachable!("opaque image has no zero initializer"),
        }
    }
    /// Writes a complete line; errors are retained in Output and checked before publication.
    fn line(&mut self, line: impl std::fmt::Display) {
        let _ = writeln!(self.out, "{line}");
    }
    /// A temporary declaration captures every argument once before a generated bounds branch.
    fn temporary(&mut self, t: &Ty, value: Option<&str>) -> String {
        let id = format!("nc_t{}", self.temp);
        self.temp += 1;
        let value = value.map_or_else(|| self.zero(t), str::to_owned);
        let declaration = self.decl(t, &id);
        self.line(if self.gl {
            format!("{declaration} = {value};")
        } else {
            format!("var {declaration} = {value};")
        });
        id
    }
    /// Interface qualifiers are emitted only on WGSL entry parameters and structure members.
    fn attribute(&self, binding: &Option<Binding>) -> String {
        match binding {
            Some(Binding::Builtin(b)) => format!(
                "@builtin({}) ",
                match b {
                    BuiltinIo::Position => "position",
                    BuiltinIo::VertexIndex => "vertex_index",
                    BuiltinIo::InstanceIndex => "instance_index",
                    BuiltinIo::FrontFacing => "front_facing",
                }
            ),
            Some(Binding::Location(n, i, s)) => format!(
                "@location({n}) {}",
                if *i == ShaderInterpolation::Flat {
                    "@interpolate(flat) ".into()
                } else {
                    format!(
                        "@interpolate(perspective, {}) ",
                        if *s == ShaderSampling::Centroid { "centroid" } else { "center" }
                    )
                }
            ),
            None => String::new(),
        }
    }
    /// Emits structures, selected resources, reachable functions, and one native entry wrapper.
    fn module(&mut self, index: usize) {
        if self.gl {
            self.line("#version 300 es\nprecision highp float;\nprecision highp int;");
        }
        for (i, s) in self.m.structures.iter().enumerate() {
            self.line(format!("struct nc_s{i} {{"));
            for (j, f) in s.fields.iter().enumerate() {
                let declaration = self.decl(&f.ty, &format!("m{j}"));
                self.line(if self.gl {
                    format!("{declaration};")
                } else {
                    format!("{}{declaration},", self.attribute(&f.binding))
                });
            }
            self.line(if self.gl { "};" } else { "}" });
        }
        let f = &self.m.functions[index];
        for &i in &f.resources {
            let r = &self.m.resources[i];
            let group = if r.stage == GpuShaderStage::Vertex { 0 } else { 2 };
            if r.ty == Ty::Texture {
                if self.gl {
                    self.line(format!("uniform sampler2D {};", texture_name(r)));
                } else {
                    self.line(format!(
                        "@group({group}) @binding({}) var nc_r{i}: texture_2d<f32>;\n@group({group}) @binding({}) var nc_sampler{i}: sampler;",
                        r.slot * 2,
                        r.slot * 2 + 1
                    ));
                }
            } else if self.gl {
                self.line(format!("layout(std140) uniform {} {{ {} nc_r{i}; }};", block_name(r), self.ty(&r.ty)));
            } else {
                self.line(format!("@group({}) @binding({}) var<uniform> nc_r{i}: {};", group + 1, r.slot, self.ty(&r.ty)));
            }
        }
        if self.gl {
            self.interface_declarations(index);
        }
        for i in reachable(self.m, index) {
            self.function(i, index);
        }
        if self.gl {
            self.wrapper(index);
        }
    }
    /// Helper signatures have no entry-only WGSL attributes.
    fn function(&mut self, index: usize, selected: usize) {
        self.function = index;
        self.temp = 0;
        let f = &self.m.functions[index];
        let params = f
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "{}{}",
                    if !self.gl && index == selected {
                        self.attribute(&p.binding)
                    } else {
                        String::new()
                    },
                    self.decl(&p.ty, &format!("v{i}"))
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        if self.gl {
            self.line(format!("{} nc_f{index}({params}) {{", self.ty(&f.result.ty)));
        } else {
            let decoration = if index == selected {
                if f.stage == Some(GpuShaderStage::Vertex) { "@vertex " } else { "@fragment " }
            } else {
                ""
            };
            let result = if f.result.ty == Ty::Void {
                String::new()
            } else {
                format!(
                    " -> {}{}",
                    if index == selected {
                        self.attribute(&f.result.binding)
                    } else {
                        String::new()
                    },
                    self.ty(&f.result.ty)
                )
            };
            self.line(format!(
                "{decoration}fn {}({params}){result} {{",
                if index == selected { f.name.clone() } else { format!("nc_f{index}") }
            ));
        }
        self.statements(&f.body);
        self.line("}");
    }
    /// Structured statements retain lexical scopes and loop re-evaluation of lowered expressions.
    fn statements(&mut self, statements: &[Stmt]) {
        for statement in statements {
            match statement {
                Stmt::Let(i, value) => {
                    let local = &self.m.functions[self.function].locals[*i];
                    let value = if let Some(e) = value { self.expr(e) } else { self.zero(&local.ty) };
                    let decl = self.decl(&local.ty, &format!("v{i}"));
                    self.line(if self.gl {
                        format!("{decl} = {value};")
                    } else {
                        format!("{} {decl} = {value};", if local.mutable { "var" } else { "let" })
                    });
                }
                Stmt::Store(target, value) => {
                    let value = self.expr(value);
                    let mut guards = Vec::new();
                    let target = self.lvalue(target, &mut guards);
                    if !guards.is_empty() {
                        self.line(format!("if ({}) {{", guards.join(" && ")));
                    }
                    self.line(format!("{target} = {value};"));
                    if !guards.is_empty() {
                        self.line("}");
                    }
                }
                Stmt::Return(value) => {
                    let value = value.as_ref().map(|e| self.expr(e));
                    self.line(format!("return{};", value.map_or(String::new(), |s| format!(" {s}"))));
                }
                Stmt::If(condition, yes, no) => {
                    let c = self.expr(condition);
                    self.line(format!("if ({c}) {{"));
                    self.statements(yes);
                    if !no.is_empty() {
                        self.line("} else {");
                        self.statements(no);
                    }
                    self.line("}");
                }
                Stmt::While(condition, body) => {
                    self.line(if self.gl { "while (true) {" } else { "loop {" });
                    let c = self.expr(condition);
                    self.line(format!("if (!({c})) {{ break; }}"));
                    self.statements(body);
                    self.line("}");
                }
                Stmt::Break => self.line("break;"),
                Stmt::Continue => self.line("continue;"),
            }
        }
    }
    /// Index guards use unsigned comparisons to reject negative and oversized indices together.
    fn index(&mut self, index: &Expr, count: u32) -> (String, String) {
        let value = self.expr(index);
        let value = self.temporary(&index.ty, Some(&value));
        let cast = if self.gl { "uint" } else { "u32" };
        (value.clone(), format!("{cast}({value}) < {count}u"))
    }
    /// Writable paths collect bounds predicates; no indexed pointer is formed before admission.
    fn lvalue(&mut self, e: &Expr, guards: &mut Vec<String>) -> String {
        match &e.kind {
            ExprKind::Local(i) => format!("v{i}"),
            ExprKind::Field(base, i) => format!("{}.m{i}", self.lvalue(base, guards)),
            ExprKind::Swizzle(base, lanes) => format!("{}.{}", self.lvalue(base, guards), Self::swizzle(lanes)),
            ExprKind::Index(base, index, count) => {
                let base = self.lvalue(base, guards);
                let (index, guard) = self.index(index, *count);
                guards.push(guard);
                format!("{base}[{index}]")
            }
            _ => unreachable!("validated lvalue"),
        }
    }
    /// Swizzle spelling always uses one alphabet for both target languages.
    fn swizzle(lanes: &[u8]) -> String {
        lanes.iter().map(|i| b"xyzw"[*i as usize] as char).collect()
    }
    /// Emits pure expressions, materializing guards before any unsafe native indexing/load.
    fn expr(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Literal(l) => match l {
                Literal::Float(v) => {
                    let s = v.to_string();
                    if s.contains(['.', 'e', 'E']) { s } else { format!("{s}.0") }
                }
                Literal::Sint(v) => v.to_string(),
                Literal::Uint(v) => format!("{v}u"),
                Literal::Bool(v) => v.to_string(),
            },
            ExprKind::Local(i) => format!("v{i}"),
            ExprKind::Resource(i) => {
                if self.gl && e.ty == Ty::Texture {
                    texture_name(&self.m.resources[*i])
                } else {
                    format!("nc_r{i}")
                }
            }
            ExprKind::Field(base, i) => format!("({}).m{i}", self.expr(base)),
            ExprKind::Swizzle(base, lanes) => format!("({}).{}", self.expr(base), Self::swizzle(lanes)),
            ExprKind::Index(base, index, count) => {
                let b = self.expr(base);
                let (i, guard) = self.index(index, *count);
                let result = self.temporary(&e.ty, None);
                self.line(format!("if ({guard}) {{ {result} = ({b})[{i}]; }}"));
                result
            }
            ExprKind::Unary(Unary::Not, a) if self.gl && a.ty.lanes() > 1 => format!("not({})", self.expr(a)),
            ExprKind::Unary(op, a) => format!(
                "({}{})",
                match op {
                    Unary::Negate => "-",
                    Unary::Not => "!",
                    Unary::Complement => "~",
                },
                self.expr(a)
            ),
            ExprKind::Binary(op, left, right) if matches!(op, Binary::And | Binary::Or) => {
                let first = self.expr(left);
                let result = self.temporary(&e.ty, Some(&first));
                let condition = if *op == Binary::And { result.clone() } else { format!("!{result}") };
                self.line(format!("if ({condition}) {{"));
                let second = self.expr(right);
                self.line(format!("{result} = {second};"));
                self.line("}");
                result
            }
            ExprKind::Binary(op, a, b) => {
                let mut left = self.expr(a);
                let mut right = self.expr(b);
                if !self.gl
                    && *op == Binary::Divide
                    && let Ty::Matrix(columns, _) = e.ty
                {
                    // WGSL divides vectors by scalars, but has no matrix division.
                    // Capture operands once and rebuild the result by columns.
                    let left = self.temporary(&a.ty, Some(&left));
                    let right = self.temporary(&b.ty, Some(&right));
                    let columns = (0..columns).map(|i| format!("{left}[{i}] / {right}")).collect::<Vec<_>>();
                    return format!("{}({})", self.ty(&e.ty), columns.join(", "));
                }
                if matches!(a.ty, Ty::Scalar(_)) && b.ty.lanes() > 1 {
                    left = format!("{}({left})", self.ty(&b.ty));
                }
                if matches!(b.ty, Ty::Scalar(_)) && a.ty.lanes() > 1 {
                    right = format!("{}({right})", self.ty(&a.ty));
                }
                if self.gl && matches!(e.ty, Ty::Vector(Scalar::Bool, _)) {
                    format!(
                        "{}({left}, {right})",
                        match op {
                            Binary::Equal => "equal",
                            Binary::NotEqual => "notEqual",
                            Binary::Less => "lessThan",
                            Binary::LessEqual => "lessThanEqual",
                            Binary::Greater => "greaterThan",
                            Binary::GreaterEqual => "greaterThanEqual",
                            _ => unreachable!(),
                        }
                    )
                } else if self.gl && *op == Binary::Remainder && a.ty.scalar() == Some(Scalar::Float) {
                    format!("({left} - {right} * trunc({left} / {right}))")
                } else {
                    format!(
                        "({left} {} {right})",
                        match op {
                            Binary::Add => "+",
                            Binary::Subtract => "-",
                            Binary::Multiply => "*",
                            Binary::Divide => "/",
                            Binary::Remainder => "%",
                            Binary::Equal => "==",
                            Binary::NotEqual => "!=",
                            Binary::Less => "<",
                            Binary::LessEqual => "<=",
                            Binary::Greater => ">",
                            Binary::GreaterEqual => ">=",
                            Binary::And => "&&",
                            Binary::Or => "||",
                            Binary::BitAnd => "&",
                            Binary::BitOr => "|",
                            Binary::Xor => "^",
                            Binary::ShiftLeft => "<<",
                            Binary::ShiftRight => ">>",
                        }
                    )
                }
            }
            ExprKind::Construct(args) => {
                let args = args.iter().map(|e| self.expr(e)).collect::<Vec<_>>();
                format!("{}({})", self.ty(&e.ty), args.join(", "))
            }
            ExprKind::Call(i, args) => {
                let args = args.iter().map(|e| self.expr(e)).collect::<Vec<_>>();
                format!("nc_f{i}({})", args.join(", "))
            }
            ExprKind::Builtin(op, args) => self.intrinsic(*op, args, &e.ty),
        }
    }
    /// Image operations use explicit mip zero and correct paired resource names.
    fn intrinsic(&mut self, op: Builtin, args: &[Expr], ty: &Ty) -> String {
        use Builtin::*;
        let mut a = args.iter().map(|e| self.expr(e)).collect::<Vec<_>>();
        if matches!(op, Sample | SampleLevel | Load | Dimensions) {
            let i = match args[0].kind {
                ExprKind::Resource(i) => i,
                _ => unreachable!(),
            };
            let image = a.remove(0);
            match op {
                Sample => {
                    return if self.gl {
                        format!("texture({image}, {})", a[0])
                    } else {
                        format!("textureSample({image}, nc_sampler{i}, {})", a[0])
                    };
                }
                SampleLevel => {
                    return if self.gl {
                        format!("textureLod({image}, {}, 0.0)", a[0])
                    } else {
                        format!("textureSampleLevel({image}, nc_sampler{i}, {}, 0.0)", a[0])
                    };
                }
                Dimensions => {
                    return if self.gl {
                        format!("uvec2(textureSize({image}, 0))")
                    } else {
                        format!("textureDimensions({image}, 0)")
                    };
                }
                Load => {
                    let coordinate = self.temporary(&args[1].ty, Some(&a[0]));
                    let result = self.temporary(ty, None);
                    let all = if self.gl {
                        format!("all(lessThan(uvec2({coordinate}), uvec2(textureSize({image}, 0))))")
                    } else {
                        format!("all(vec2<u32>({coordinate}) < textureDimensions({image}, 0))")
                    };
                    let load = if self.gl {
                        format!("texelFetch({image}, ivec2({coordinate}), 0)")
                    } else {
                        format!("textureLoad({image}, vec2<i32>({coordinate}), 0)")
                    };
                    self.line(format!("if ({all}) {{ {result} = {load}; }}"));
                    return result;
                }
                _ => unreachable!(),
            }
        }
        if op == Select {
            if !self.gl {
                return format!("select({}, {}, {})", a[0], a[1], a[2]);
            }
            // GLSL ternaries cover float, integer, and boolean selections. Materialize
            // each argument first to preserve eager evaluation and derivative uniformity.
            for (value, argument) in a.iter_mut().zip(args) {
                *value = self.temporary(&argument.ty, Some(value));
            }
            if args[2].ty.lanes() == 1 {
                return format!("({} ? {} : {})", a[2], a[1], a[0]);
            }
            let lanes = (0..ty.lanes())
                .map(|i| {
                    let lane = b"xyzw"[i as usize] as char;
                    format!("({}.{lane} ? {}.{lane} : {}.{lane})", a[2], a[1], a[0])
                })
                .collect::<Vec<_>>();
            return format!("{}({})", self.ty(ty), lanes.join(", "));
        }
        let name = match op {
            Abs => "abs",
            Min => "min",
            Max => "max",
            Clamp => "clamp",
            Floor => "floor",
            Ceil => "ceil",
            Fract => "fract",
            Round => {
                if self.gl {
                    "roundEven"
                } else {
                    "round"
                }
            }
            Trunc => "trunc",
            Sqrt => "sqrt",
            InverseSqrt => {
                if self.gl {
                    "inversesqrt"
                } else {
                    "inverseSqrt"
                }
            }
            Sin => "sin",
            Cos => "cos",
            Tan => "tan",
            Asin => "asin",
            Acos => "acos",
            Atan => "atan",
            Atan2 => {
                if self.gl {
                    "atan"
                } else {
                    "atan2"
                }
            }
            Exp => "exp",
            Exp2 => "exp2",
            Log => "log",
            Log2 => "log2",
            Pow => "pow",
            Dot => "dot",
            Cross => "cross",
            Normalize => "normalize",
            Length => "length",
            Distance => "distance",
            Reflect => "reflect",
            Mix => "mix",
            Dpdx => {
                if self.gl {
                    "dFdx"
                } else {
                    "dpdx"
                }
            }
            Dpdy => {
                if self.gl {
                    "dFdy"
                } else {
                    "dpdy"
                }
            }
            Fwidth => "fwidth",
            All => "all",
            Any => "any",
            _ => unreachable!(),
        };
        format!("{name}({})", a.join(", "))
    }
    /// Recursively flattens structures to bound IO leaves, retaining their member path.
    fn leaves(&self, field: &Field, path: String, out: &mut Vec<(Field, String)>) {
        if field.binding.is_some() {
            out.push((field.clone(), path));
        } else if let Ty::Struct(i) = field.ty {
            for (j, f) in self.m.structures[i].fields.iter().enumerate() {
                self.leaves(f, format!("{path}.m{j}"), out);
            }
        }
    }
    /// All ES300 locations use stage-independent varying names for native linker agreement.
    fn interface_declarations(&mut self, index: usize) {
        let f = &self.m.functions[index];
        let stage = f.stage.unwrap();
        for input in [true, false] {
            let mut leaves = Vec::new();
            let fields = if input { f.params.as_slice() } else { std::slice::from_ref(&f.result) };
            for field in fields {
                self.leaves(field, String::new(), &mut leaves);
            }
            for (field, _) in leaves {
                if let Some(Binding::Location(n, i, s)) = field.binding {
                    let vertex_input = stage == GpuShaderStage::Vertex && input;
                    let fragment_output = stage == GpuShaderStage::Fragment && !input;
                    let name = if vertex_input {
                        format!("nc_attribute{n}")
                    } else if fragment_output {
                        format!("nc_color{n}")
                    } else {
                        format!("nc_varying{n}")
                    };
                    let location = if vertex_input || fragment_output {
                        format!("layout(location={n}) ")
                    } else {
                        String::new()
                    };
                    let interpolation = if vertex_input || fragment_output {
                        ""
                    } else if i == ShaderInterpolation::Flat {
                        "flat "
                    } else if s == ShaderSampling::Centroid {
                        "centroid "
                    } else {
                        ""
                    };
                    self.line(format!(
                        "{location}{interpolation}{} {};",
                        if input { "in" } else { "out" },
                        self.decl(&field.ty, &name)
                    ));
                }
            }
        }
    }
    /// Converts builtin names without depending on textual epilogue substitutions.
    fn native_io(&self, binding: &Binding, stage: GpuShaderStage, input: bool) -> String {
        match binding {
            Binding::Builtin(b) => match b {
                BuiltinIo::Position => if input { "gl_FragCoord" } else { "gl_Position" }.into(),
                BuiltinIo::VertexIndex => "uint(gl_VertexID)".into(),
                BuiltinIo::InstanceIndex => "uint(gl_InstanceID)".into(),
                BuiltinIo::FrontFacing => "gl_FrontFacing".into(),
            },
            Binding::Location(n, ..) => {
                if stage == GpuShaderStage::Vertex && input {
                    format!("nc_attribute{n}")
                } else if stage == GpuShaderStage::Fragment && !input {
                    format!("nc_color{n}")
                } else {
                    format!("nc_varying{n}")
                }
            }
        }
    }
    /// Standard main assembles arguments, calls the selected function, and converts clip space once.
    fn wrapper(&mut self, index: usize) {
        let f = &self.m.functions[index];
        let stage = f.stage.unwrap();
        self.line("void main() {");
        for (i, param) in f.params.iter().enumerate() {
            let name = format!("nc_arg{i}");
            self.line(format!("{} = {};", self.decl(&param.ty, &name), self.zero(&param.ty)));
            let mut leaves = Vec::new();
            self.leaves(param, name, &mut leaves);
            for (leaf, path) in leaves {
                self.line(format!("{path} = {};", self.native_io(leaf.binding.as_ref().unwrap(), stage, true)));
            }
        }
        self.line(format!(
            "{} nc_result = nc_f{index}({});",
            self.ty(&f.result.ty),
            (0..f.params.len()).map(|i| format!("nc_arg{i}")).collect::<Vec<_>>().join(", ")
        ));
        let mut leaves = Vec::new();
        self.leaves(&f.result, "nc_result".into(), &mut leaves);
        for (leaf, path) in leaves {
            self.line(format!("{} = {path};", self.native_io(leaf.binding.as_ref().unwrap(), stage, false)));
        }
        if stage == GpuShaderStage::Vertex {
            self.line("gl_Position.y = -gl_Position.y;\ngl_Position.z = 2.0 * gl_Position.z - gl_Position.w;");
        }
        self.line("}");
    }
}
