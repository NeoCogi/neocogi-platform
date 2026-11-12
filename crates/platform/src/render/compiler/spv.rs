//! Direct SPIR-V 1.0 emission for Vulkan. No other shader compiler is linked.
use super::{CompileError, access, ir::*, text::reachable};
use neocogi_platform_contracts::render::{
    abi::GpuShaderStage,
    shader::{ShaderInterpolation, ShaderSampling},
};
use spirv::{Op, StorageClass as Storage, Decoration as Dec, BuiltIn as Io, GlslStd450Op as Glsl};
use std::collections::BTreeMap;

/// Encodes a complete instruction with a bounded, compile-time-selected opcode.
fn instruction(out: &mut Vec<u32>, op: Op, args: &[u32]) {
    out.push((((args.len() + 1) as u32) << 16) | op as u32);
    out.extend_from_slice(args);
}
/// SPIR-V strings are null-terminated UTF-8 padded into little-endian words.
fn string(value: &str) -> Vec<u32> {
    let mut bytes = value.as_bytes().to_vec();
    bytes.push(0);
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    bytes.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
}
/// Native storage has distinct uniform and sampled-image contracts; no sentinel IDs are used.
#[derive(Clone, Copy)]
enum NativeResource {
    /// Descriptor points to a block wrapper containing the source uniform structure.
    Uniform { variable: u32 },
    /// Independently decorated image and sampler descriptors form one admitted source texture.
    Texture { image: u32, sampler: u32 },
}
/// Bounds facts compose across nested paths without forming an invalid pointer.
#[derive(Clone, Copy)]
enum Guard {
    /// Every component is statically in bounds.
    Always,
    /// A constant component is out of bounds; index expressions still evaluate.
    Never,
    /// One scalar boolean result dominates the eventual read or write.
    Dynamic(u32),
}
/// Native projection retains both pointer indices and SSA extraction information.
#[derive(Clone)]
struct Projection {
    /// Integer SSA value consumed by a native access chain or dynamic vector extraction.
    index: u32,
    /// Literal member/index, when legal for OpCompositeExtract.
    constant: Option<u32>,
    /// Value type after this projection, needed when walking SSA composites.
    ty: Ty,
}
/// Resolved declaration origin; storage classes never use sentinel pointer IDs.
#[derive(Clone, Copy)]
enum Origin {
    /// Immutable scalar/vector or a structure without address-requiring members.
    Value(u32),
    /// Uniform or function storage; access chains are delayed until guards succeed.
    Pointer { variable: u32, storage: Storage },
}
/// Captured read-only path; immutable uniform aliases retain evaluated indices, not a palette copy.
#[derive(Clone)]
struct Access {
    /// Original value or declaration, preserving its native storage class.
    origin: Origin,
    /// All projections, including captured indices of a uniform alias.
    projections: Vec<Projection>,
    /// Conjunction of bounds checks for the complete leaf selection.
    guard: Guard,
    /// Contents cannot change during this invocation; immutable aliases may share this path.
    stable: bool,
}
/// A local is bound once at its lexical declaration; mutable bindings own one function variable.
#[derive(Clone)]
enum LocalValue {
    /// Not yet declared in the current lexical traversal; validation excludes reads in this state.
    Undeclared,
    /// An SSA value, owned local storage, or an immutable captured uniform path.
    Bound(Access),
}
/// One logical module with globally unique IDs and ordered SPIR-V sections.
struct Writer<'a> {
    /// Validated module borrowed for this one compilation.
    m: &'a Module,
    /// Next globally unique SPIR-V result ID.
    next: u32,
    /// Canonical source-value types mapped to native type IDs.
    types: BTreeMap<Ty, u32>,
    /// Storage-class and pointee-ID pairs deduplicate native pointer types.
    pointers: BTreeMap<(u32, u32), u32>,
    /// Typed scalar bit patterns deduplicate literal constants.
    constants: BTreeMap<(Ty, u32), u32>,
    /// Typed null constants initialize locals and rejected reads.
    zeros: BTreeMap<Ty, u32>,
    /// Types, constants, and globals in dependency order.
    declarations: Vec<u32>,
    /// Decorations precede declarations in final logical module order.
    annotations: Vec<u32>,
    /// Complete helper and wrapper definitions, ordered callee before caller.
    functions: Vec<u32>,
    /// Native input/output variable IDs listed in the selected entry declaration.
    entry: Vec<u32>,
    /// Explicit native uniform or image/sampler storage by module resource index.
    resources: BTreeMap<usize, NativeResource>,
    /// Stable native function IDs permit forward signature resolution.
    function_ids: Vec<u32>,
    /// Result/parameter type sequences deduplicate nonaggregate function types.
    function_types: BTreeMap<Vec<u32>, u32>,
    /// Selected execution stage determines legal native IO decorations.
    stage: GpuShaderStage,
    /// Native sampled 2D floating-point image type ID.
    image: u32,
    /// Native ordinary sampler type ID.
    sampler: u32,
    /// Native combined image/sampler value type ID.
    sampled_image: u32,
    /// Imported GLSL.std.450 instruction set ID.
    ext: u32,
    /// Maximum selected binary bytes, excluding the already charged entry name.
    limit: usize,
}
/// Per-function instructions and prologue variables remain separate until assembly.
struct Body {
    /// Function-storage declarations inserted into the entry block prologue.
    variables: Vec<u32>,
    /// Basic blocks and executable instructions, excluding the prologue.
    code: Vec<u32>,
    /// Resolved local values and storage; immutable values need no store/load round trip.
    locals: Vec<LocalValue>,
    /// Pointee types mirror the local pointer table for zero initialization.
    local_types: Vec<Ty>,
    /// A terminator has closed the current block; a label opens the next one.
    closed: bool,
    /// Last emitted label, used to join guarded values with SSA Phi instructions.
    label: u32,
    /// Enclosing merge/continue labels, used by typed loop-control statements.
    loops: Vec<(u32, u32)>,
}
impl Body {
    /// A terminator closes one basic block; opening a label restores instruction admission.
    fn op(&mut self, op: Op, args: &[u32]) {
        instruction(&mut self.code, op, args);
        if matches!(op, Op::Return | Op::ReturnValue | Op::Branch | Op::BranchConditional | Op::Unreachable) {
            self.closed = true;
        } else if op == Op::Label {
            self.closed = false;
            self.label = args[0];
        }
    }
}
/// Emits one entry without retaining another target or relying on runtime external tools.
pub(crate) fn emit(m: &Module, index: usize, limit: usize) -> Result<Vec<u32>, CompileError> {
    let mut w = Writer {
        m,
        next: 1,
        types: BTreeMap::new(),
        pointers: BTreeMap::new(),
        constants: BTreeMap::new(),
        zeros: BTreeMap::new(),
        declarations: Vec::new(),
        annotations: Vec::new(),
        functions: Vec::new(),
        entry: Vec::new(),
        resources: BTreeMap::new(),
        function_ids: Vec::new(),
        function_types: BTreeMap::new(),
        stage: m.functions[index].stage.unwrap(),
        image: 0,
        sampler: 0,
        sampled_image: 0,
        ext: 0,
        limit,
    };
    w.module(index)
}
impl Writer<'_> {
    /// IDs are bounded by the frontend's node count and emitted payload cap.
    fn id(&mut self) -> u32 {
        let id = self.next;
        self.next += 1;
        id
    }
    /// Emits a typed result into an existing basic block.
    fn value(&mut self, b: &mut Body, op: Op, ty: &Ty, args: &[u32]) -> u32 {
        let ty = self.ty(ty);
        self.raw_value(b, op, ty, args)
    }
    /// Native image/sampler values have types outside the source language value set.
    fn raw_value(&mut self, b: &mut Body, op: Op, ty: u32, args: &[u32]) -> u32 {
        let id = self.id();
        let mut operands = vec![ty, id];
        operands.extend_from_slice(args);
        b.op(op, &operands);
        id
    }
    /// Deduplicates every finite value type; member layout comes from frontend reflection.
    fn ty(&mut self, t: &Ty) -> u32 {
        if let Some(&id) = self.types.get(t) {
            return id;
        }
        let (op, args) = match t {
            Ty::Void => (Op::TypeVoid, vec![]),
            Ty::Texture => unreachable!("image types use native resource declarations"),
            Ty::Scalar(Scalar::Bool) => (Op::TypeBool, vec![]),
            Ty::Scalar(Scalar::Float) => (Op::TypeFloat, vec![32]),
            Ty::Scalar(s) => (Op::TypeInt, vec![32, u32::from(*s == Scalar::Sint)]),
            Ty::Vector(s, n) => (Op::TypeVector, vec![self.ty(&Ty::Scalar(*s)), u32::from(*n)]),
            Ty::Matrix(c, r) => (Op::TypeMatrix, vec![self.ty(&Ty::Vector(Scalar::Float, *r)), u32::from(*c)]),
            Ty::Array(base, n) => (Op::TypeArray, vec![self.ty(base), self.constant(&Ty::Scalar(Scalar::Uint), *n)]),
            Ty::Struct(i) => (Op::TypeStruct, self.m.structures[*i].fields.iter().map(|f| self.ty(&f.ty)).collect()),
        };
        let id = self.id();
        let mut operands = vec![id];
        operands.extend(args);
        instruction(&mut self.declarations, op, &operands);
        self.types.insert(t.clone(), id);
        match t {
            Ty::Array(base, _) => {
                let (a, s) = self.m.layout(base);
                self.decorate(id, Dec::ArrayStride, &[s.div_ceil(a) * a]);
            }
            Ty::Struct(i) => {
                for (j, offset) in self.m.offsets(*i).into_iter().enumerate() {
                    self.member(id, j as u32, Dec::Offset, &[offset]);
                    if let Some(stride) = self.matrix_stride(&self.m.structures[*i].fields[j].ty) {
                        self.member(id, j as u32, Dec::ColMajor, &[]);
                        self.member(id, j as u32, Dec::MatrixStride, &[stride]);
                    }
                }
            }
            _ => {}
        }
        id
    }
    /// Arrays of matrices carry their matrix layout on the containing structure member.
    fn matrix_stride(&self, t: &Ty) -> Option<u32> {
        match t {
            Ty::Matrix(_, r) => {
                let (a, s) = self.m.layout(&Ty::Vector(Scalar::Float, *r));
                Some(s.div_ceil(a) * a)
            }
            Ty::Array(t, _) => self.matrix_stride(t),
            _ => None,
        }
    }
    /// Pointer types are deduplicated by storage class and exact pointee ID.
    fn pointer(&mut self, storage: Storage, ty: u32) -> u32 {
        let key = (storage as u32, ty);
        if let Some(&id) = self.pointers.get(&key) {
            return id;
        }
        let id = self.id();
        instruction(&mut self.declarations, Op::TypePointer, &[id, storage as u32, ty]);
        self.pointers.insert(key, id);
        id
    }
    /// Scalar constants preserve bits; booleans use their dedicated instructions.
    fn constant(&mut self, t: &Ty, bits: u32) -> u32 {
        let key = (t.clone(), bits);
        if let Some(&id) = self.constants.get(&key) {
            return id;
        }
        let ty = self.ty(t);
        let id = self.id();
        if *t == Ty::Scalar(Scalar::Bool) {
            instruction(&mut self.declarations, if bits == 0 { Op::ConstantFalse } else { Op::ConstantTrue }, &[ty, id]);
        } else {
            instruction(&mut self.declarations, Op::Constant, &[ty, id, bits]);
        }
        self.constants.insert(key, id);
        id
    }
    /// Null composite constants initialize private state and out-of-range reads without expansion.
    fn zero(&mut self, t: &Ty) -> u32 {
        if let Some(&id) = self.zeros.get(t) {
            return id;
        }
        let ty = self.ty(t);
        let id = self.id();
        instruction(&mut self.declarations, Op::ConstantNull, &[ty, id]);
        self.zeros.insert(t.clone(), id);
        id
    }
    /// Adds a decoration to the annotation section, before all declarations.
    fn decorate(&mut self, id: u32, d: Dec, args: &[u32]) {
        let mut a = vec![id, d as u32];
        a.extend_from_slice(args);
        instruction(&mut self.annotations, Op::Decorate, &a);
    }
    /// Adds a structure-member decoration using authoritative frontend offsets.
    fn member(&mut self, id: u32, index: u32, d: Dec, args: &[u32]) {
        let mut a = vec![id, index, d as u32];
        a.extend_from_slice(args);
        instruction(&mut self.annotations, Op::MemberDecorate, &a);
    }
    /// Global variable creation has an explicit typed storage class.
    fn global(&mut self, t: u32, storage: Storage) -> u32 {
        let p = self.pointer(storage, t);
        let id = self.id();
        instruction(&mut self.declarations, Op::Variable, &[p, id, storage as u32]);
        id
    }
    /// Function scratch variables are inserted in the first block before executable instructions.
    fn variable(&mut self, b: &mut Body, t: &Ty) -> u32 {
        let ty = self.ty(t);
        let p = self.pointer(Storage::Function, ty);
        let id = self.id();
        instruction(&mut b.variables, Op::Variable, &[p, id, Storage::Function as u32]);
        id
    }
    /// Assembles Vulkan resources and a selected entry, preserving SPIR-V logical section order.
    fn module(&mut self, index: usize) -> Result<Vec<u32>, CompileError> {
        self.ext = self.id();
        let mut imports = vec![self.ext];
        imports.extend(string("GLSL.std.450"));
        let f = &self.m.functions[index];
        let float = self.ty(&Ty::Scalar(Scalar::Float));
        self.image = self.id();
        instruction(
            &mut self.declarations,
            Op::TypeImage,
            &[self.image, float, spirv::Dim::Dim2D as u32, 0, 0, 0, 1, spirv::ImageFormat::Unknown as u32],
        );
        self.sampler = self.id();
        instruction(&mut self.declarations, Op::TypeSampler, &[self.sampler]);
        self.sampled_image = self.id();
        instruction(&mut self.declarations, Op::TypeSampledImage, &[self.sampled_image, self.image]);
        for &i in &f.resources {
            let r = &self.m.resources[i];
            let group = if r.stage == GpuShaderStage::Vertex { 0 } else { 2 };
            let pair = if r.ty == Ty::Texture {
                let image = self.global(self.image, Storage::UniformConstant);
                let sampler = self.global(self.sampler, Storage::UniformConstant);
                self.decorate(image, Dec::DescriptorSet, &[group]);
                self.decorate(image, Dec::Binding, &[r.slot * 2]);
                self.decorate(sampler, Dec::DescriptorSet, &[group]);
                self.decorate(sampler, Dec::Binding, &[r.slot * 2 + 1]);
                NativeResource::Texture { image, sampler }
            } else {
                let ty = self.ty(&r.ty);
                let wrapper = self.id();
                instruction(&mut self.declarations, Op::TypeStruct, &[wrapper, ty]);
                self.decorate(wrapper, Dec::Block, &[]);
                self.member(wrapper, 0, Dec::Offset, &[0]);
                let variable = self.global(wrapper, Storage::Uniform);
                self.decorate(variable, Dec::DescriptorSet, &[group + 1]);
                self.decorate(variable, Dec::Binding, &[r.slot]);
                NativeResource::Uniform { variable }
            };
            self.resources.insert(i, pair);
        }
        self.function_ids = (0..self.m.functions.len()).map(|_| self.id()).collect();
        for i in reachable(self.m, index) {
            self.function(i);
            self.check_limit(0)?;
        }
        let wrapper = self.wrapper(index);
        let model = if f.stage == Some(GpuShaderStage::Vertex) {
            spirv::ExecutionModel::Vertex
        } else {
            spirv::ExecutionModel::Fragment
        };
        let mut entry = vec![model as u32, wrapper];
        entry.extend(string(&f.name));
        entry.extend_from_slice(&self.entry);
        self.check_limit(0)?;
        let mut output = vec![0x07230203, 0x00010000, 0, self.next, 0];
        instruction(&mut output, Op::Capability, &[spirv::Capability::Shader as u32]);
        instruction(&mut output, Op::Capability, &[spirv::Capability::ImageQuery as u32]);
        instruction(&mut output, Op::ExtInstImport, &imports);
        instruction(
            &mut output,
            Op::MemoryModel,
            &[spirv::AddressingModel::Logical as u32, spirv::MemoryModel::GLSL450 as u32],
        );
        instruction(&mut output, Op::EntryPoint, &entry);
        if f.stage == Some(GpuShaderStage::Fragment) {
            instruction(&mut output, Op::ExecutionMode, &[wrapper, spirv::ExecutionMode::OriginUpperLeft as u32]);
        }
        output.extend_from_slice(&self.annotations);
        output.extend_from_slice(&self.declarations);
        output.extend_from_slice(&self.functions);
        if output.len() > self.limit / 4 {
            return Err(CompileError::Limit);
        }
        Ok(output)
    }
    /// Bounds scratch sections before final concatenation; final exact bytes are checked again.
    fn check_limit(&self, extra: usize) -> Result<(), CompileError> {
        if self.annotations.len() + self.declarations.len() + self.functions.len() + extra > self.limit / 4 {
            Err(CompileError::Limit)
        } else {
            Ok(())
        }
    }
    /// Function types are nonaggregate SPIR-V types and must be deduplicated too.
    fn function_type(&mut self, signature: &[u32]) -> u32 {
        if let Some(&id) = self.function_types.get(signature) {
            return id;
        }
        let id = self.id();
        let mut args = vec![id];
        args.extend_from_slice(signature);
        instruction(&mut self.declarations, Op::TypeFunction, &args);
        self.function_types.insert(signature.to_vec(), id);
        id
    }

    /// Encodes an ordinary function; only its wrapper is exposed as a SPIR-V entry.
    fn function(&mut self, index: usize) {
        let f = &self.m.functions[index];
        let result = self.ty(&f.result.ty);
        let parameters = f.params.iter().map(|p| self.ty(&p.ty)).collect::<Vec<_>>();
        let mut signature = vec![result];
        signature.extend_from_slice(&parameters);
        let function_type = self.function_type(&signature);
        let mut prefix = Vec::new();
        instruction(&mut prefix, Op::Function, &[result, self.function_ids[index], 0, function_type]);
        let mut values = Vec::new();
        for ty in parameters {
            let id = self.id();
            instruction(&mut prefix, Op::FunctionParameter, &[ty, id]);
            values.push(id);
        }
        let label = self.id();
        instruction(&mut prefix, Op::Label, &[label]);
        let mut b = Body {
            variables: Vec::new(),
            code: Vec::new(),
            locals: Vec::new(),
            local_types: Vec::new(),
            closed: false,
            label,
            loops: Vec::new(),
        };
        b.locals.resize(f.locals.len(), LocalValue::Undeclared);
        b.local_types = f.locals.iter().map(|local| local.ty.clone()).collect();
        for (i, local) in f.locals.iter().enumerate() {
            if local.mutable {
                let variable = self.variable(&mut b, &local.ty);
                b.locals[i] = LocalValue::Bound(Access {
                    origin: Origin::Pointer { variable, storage: Storage::Function },
                    projections: Vec::new(),
                    guard: Guard::Always,
                    stable: false,
                });
            }
        }
        for (i, value) in values.into_iter().enumerate() {
            b.locals[i] = LocalValue::Bound(self.owned_value(&mut b, &f.locals[i].ty, value));
        }
        self.statements(&mut b, &f.body);
        if !b.closed {
            b.op(Op::Return, &[]);
        }
        prefix.extend(b.variables);
        prefix.extend(b.code);
        instruction(&mut prefix, Op::FunctionEnd, &[]);
        self.functions.extend(prefix);
    }
    /// Structured control flow emits no instructions after a terminator in a block.
    fn statements(&mut self, b: &mut Body, statements: &[Stmt]) {
        for statement in statements {
            if b.closed {
                break;
            }
            match statement {
                Stmt::Borrow(i, expression) => {
                    let path = if let Some(path) = self.access(b, expression) {
                        path
                    } else {
                        let value = self.expr(b, expression);
                        self.owned_value(b, &expression.ty, value)
                    };
                    b.locals[*i] = LocalValue::Bound(path);
                }
                Stmt::Let(i, value) => {
                    let ty = b.local_types[*i].clone();
                    if matches!(b.locals[*i], LocalValue::Undeclared) {
                        let expression = value.as_ref().expect("immutable declarations have an initializer");
                        if let Some(path) = self.access(b, expression) {
                            // Uniform memory cannot change during an invocation. Keep the
                            // captured path, including index snapshots, instead of copying it.
                            if path.stable {
                                b.locals[*i] = LocalValue::Bound(path);
                                continue;
                            }
                            let value = self.read_access(b, &path, &ty);
                            b.locals[*i] = LocalValue::Bound(self.owned_value(b, &ty, value));
                        } else {
                            let value = self.expr(b, expression);
                            b.locals[*i] = LocalValue::Bound(self.owned_value(b, &ty, value));
                        }
                    } else {
                        let value = if let Some(e) = value { self.expr(b, e) } else { self.zero(&ty) };
                        let LocalValue::Bound(path) = b.locals[*i].clone() else { unreachable!() };
                        let pointer = self.access_pointer(b, &path, &ty);
                        b.op(Op::Store, &[pointer, value]);
                    }
                }
                Stmt::Store(target, value) => {
                    let value = self.expr(b, value);
                    let path = self.access(b, target).expect("validated local write path");
                    let merge = match path.guard {
                        Guard::Never => continue,
                        Guard::Always => None,
                        Guard::Dynamic(condition) => Some(self.begin_guard(b, condition)),
                    };
                    let pointer = self.access_pointer(b, &path, &target.ty);
                    b.op(Op::Store, &[pointer, value]);
                    if let Some(merge) = merge {
                        b.op(Op::Branch, &[merge]);
                        b.op(Op::Label, &[merge]);
                    }
                }
                Stmt::Return(value) => {
                    if let Some(e) = value {
                        let value = self.expr(b, e);
                        b.op(Op::ReturnValue, &[value]);
                    } else {
                        b.op(Op::Return, &[]);
                    }
                }
                Stmt::If(condition, yes, no) => {
                    let c = self.expr(b, condition);
                    let y = self.id();
                    let n = self.id();
                    let merge = self.id();
                    b.op(Op::SelectionMerge, &[merge, 0]);
                    b.op(Op::BranchConditional, &[c, y, n]);
                    b.op(Op::Label, &[y]);
                    self.statements(b, yes);
                    let yr = b.closed;
                    if !b.closed {
                        b.op(Op::Branch, &[merge]);
                    }
                    b.op(Op::Label, &[n]);
                    self.statements(b, no);
                    let nr = b.closed;
                    if !b.closed {
                        b.op(Op::Branch, &[merge]);
                    }
                    b.op(Op::Label, &[merge]);
                    if yr && nr {
                        b.op(Op::Unreachable, &[]);
                    }
                }
                Stmt::While(condition, body) => {
                    let header = self.id();
                    let check = self.id();
                    let body_label = self.id();
                    let continuing = self.id();
                    let merge = self.id();
                    b.op(Op::Branch, &[header]);
                    b.op(Op::Label, &[header]);
                    b.op(Op::LoopMerge, &[merge, continuing, 0]);
                    b.op(Op::Branch, &[check]);
                    b.op(Op::Label, &[check]);
                    let cond = self.expr(b, condition);
                    b.op(Op::BranchConditional, &[cond, body_label, merge]);
                    b.op(Op::Label, &[body_label]);
                    b.loops.push((merge, continuing));
                    self.statements(b, body);
                    b.loops.pop();
                    if !b.closed {
                        b.op(Op::Branch, &[continuing]);
                    }
                    b.op(Op::Label, &[continuing]);
                    b.op(Op::Branch, &[header]);
                    b.op(Op::Label, &[merge]);
                }
                Stmt::Break => b.op(Op::Branch, &[b.loops.last().unwrap().0]),
                Stmt::Continue => b.op(Op::Branch, &[b.loops.last().unwrap().1]),
            }
        }
    }
    /// Starts a single-arm selection and returns its merge label.
    fn begin_guard(&mut self, b: &mut Body, condition: u32) -> u32 {
        let yes = self.id();
        let merge = self.id();
        b.op(Op::SelectionMerge, &[merge, 0]);
        b.op(Op::BranchConditional, &[condition, yes, merge]);
        b.op(Op::Label, &[yes]);
        merge
    }
    /// Preserve immutable SSA values; aggregates requiring dynamic addressing own
    /// one storage allocation initialized at their declaration, never once per read.
    fn owned_value(&mut self, b: &mut Body, ty: &Ty, value: u32) -> Access {
        fn storage(m: &Module, ty: &Ty) -> bool {
            match ty {
                Ty::Array(..) | Ty::Matrix(..) => true,
                Ty::Struct(i) => m.structures[*i].fields.iter().any(|f| storage(m, &f.ty)),
                _ => false,
            }
        }
        let origin = if storage(self.m, ty) {
            let variable = self.variable(b, ty);
            b.op(Op::Store, &[variable, value]);
            Origin::Pointer { variable, storage: Storage::Function }
        } else {
            Origin::Value(value)
        };
        Access {
            origin,
            projections: Vec::new(),
            guard: Guard::Always,
            stable: true,
        }
    }
    /// Combine proven bounds without emitting redundant constant comparisons.
    fn and_guard(&mut self, b: &mut Body, a: Guard, c: Guard) -> Guard {
        match (a, c) {
            (Guard::Never, _) | (_, Guard::Never) => Guard::Never,
            (Guard::Always, guard) | (guard, Guard::Always) => guard,
            (Guard::Dynamic(a), Guard::Dynamic(c)) => Guard::Dynamic(self.value(b, Op::LogicalAnd, &Ty::Scalar(Scalar::Bool), &[a, c])),
        }
    }
    /// Indices evaluate once; unsigned comparisons also reject signed negatives.
    fn index_guard(&mut self, b: &mut Body, index: &Expr, count: u32) -> (u32, Guard) {
        let value = self.expr(b, index);
        if let Some(index) = access::constant_index(index) {
            return (value, if index < count { Guard::Always } else { Guard::Never });
        }
        let unsigned = if index.ty == Ty::Scalar(Scalar::Uint) {
            value
        } else {
            self.value(b, Op::Bitcast, &Ty::Scalar(Scalar::Uint), &[value])
        };
        let size = self.constant(&Ty::Scalar(Scalar::Uint), count);
        (
            value,
            Guard::Dynamic(self.value(b, Op::ULessThan, &Ty::Scalar(Scalar::Bool), &[unsigned, size])),
        )
    }
    /// Capture the complete projection chain before a branch. No pointer or
    /// aggregate load is formed while bounds can still be invalid.
    fn access(&mut self, b: &mut Body, expression: &Expr) -> Option<Access> {
        let path = access::path(expression)?;
        let mut result = match path.root {
            access::Root::Local(i) => match b.locals[i].clone() {
                LocalValue::Bound(value) => value,
                LocalValue::Undeclared => unreachable!("validated declaration dominates every use"),
            },
            access::Root::Uniform(i) => {
                let NativeResource::Uniform { variable } = self.resources[&i] else {
                    unreachable!()
                };
                Access {
                    origin: Origin::Pointer { variable, storage: Storage::Uniform },
                    projections: vec![Projection {
                        index: self.constant(&Ty::Scalar(Scalar::Uint), 0),
                        constant: Some(0),
                        ty: self.m.resources[i].ty.clone(),
                    }],
                    guard: Guard::Always,
                    stable: true,
                }
            }
        };
        for step in path.steps {
            let projection = match step {
                access::Step::Member { index, ty } => Projection {
                    index: self.constant(&Ty::Scalar(Scalar::Uint), index),
                    constant: Some(index),
                    ty: ty.clone(),
                },
                access::Step::Index { value, count, ty } => {
                    let (index, guard) = self.index_guard(b, value, count);
                    result.guard = self.and_guard(b, result.guard, guard);
                    Projection {
                        index,
                        constant: access::constant_index(value),
                        ty: ty.clone(),
                    }
                }
            };
            result.projections.push(projection);
        }
        Some(result)
    }
    /// Materialize only a proven-safe leaf pointer, retaining Uniform/Function storage.
    fn access_pointer(&mut self, b: &mut Body, path: &Access, ty: &Ty) -> u32 {
        let Origin::Pointer { variable, storage } = path.origin else {
            unreachable!("SSA values are not writable")
        };
        if path.projections.is_empty() {
            return variable;
        }
        let ty = self.ty(ty);
        let pointer = self.pointer(storage, ty);
        let mut operands = vec![variable];
        operands.extend(path.projections.iter().map(|step| step.index));
        self.raw_value(b, Op::AccessChain, pointer, &operands)
    }
    /// Load one leaf, or extract it directly from an immutable SSA parameter/value.
    fn access_value(&mut self, b: &mut Body, path: &Access, ty: &Ty) -> u32 {
        match path.origin {
            Origin::Pointer { .. } => {
                let pointer = self.access_pointer(b, path, ty);
                self.value(b, Op::Load, ty, &[pointer])
            }
            Origin::Value(mut value) => {
                for step in &path.projections {
                    value = if let Some(index) = step.constant {
                        self.value(b, Op::CompositeExtract, &step.ty, &[value, index])
                    } else {
                        self.value(b, Op::VectorExtractDynamic, &step.ty, &[value, step.index])
                    };
                }
                value
            }
        }
    }
    /// A failed nested index contributes zero without loading/copying its parent.
    /// Phi values avoid a function variable and a store/load pair for each check.
    fn read_access(&mut self, b: &mut Body, path: &Access, ty: &Ty) -> u32 {
        match path.guard {
            Guard::Always => self.access_value(b, path, ty),
            Guard::Never => self.zero(ty),
            Guard::Dynamic(condition) => {
                let zero = self.zero(ty);
                let before = b.label;
                let merge = self.begin_guard(b, condition);
                let value = self.access_value(b, path, ty);
                let after = b.label;
                b.op(Op::Branch, &[merge]);
                b.op(Op::Label, &[merge]);
                self.value(b, Op::Phi, ty, &[value, after, zero, before])
            }
        }
    }
    /// Typed expression translation shares constructor and operator rules with text writers.
    fn expr(&mut self, b: &mut Body, e: &Expr) -> u32 {
        if let Some(path) = self.access(b, e) {
            return self.read_access(b, &path, &e.ty);
        }
        match &e.kind {
            ExprKind::Literal(l) => self.constant(
                &e.ty,
                match l {
                    Literal::Float(v) => v.to_bits(),
                    Literal::Sint(v) => *v as u32,
                    Literal::Uint(v) => *v,
                    Literal::Bool(v) => u32::from(*v),
                },
            ),
            ExprKind::Local(_) => unreachable!("locals use the shared access path"),
            ExprKind::Resource(i) => match self.resources[i] {
                NativeResource::Texture { image, .. } => self.raw_value(b, Op::Load, self.image, &[image]),
                NativeResource::Uniform { .. } => unreachable!("uniforms use the shared access path"),
            },
            ExprKind::Field(base, index) => {
                let value = self.expr(b, base);
                self.value(b, Op::CompositeExtract, &e.ty, &[value, *index as u32])
            }
            ExprKind::Swizzle(base, lanes) => {
                let value = self.expr(b, base);
                if lanes.len() == 1 {
                    self.value(b, Op::CompositeExtract, &e.ty, &[value, u32::from(lanes[0])])
                } else {
                    let mut args = vec![value, value];
                    args.extend(lanes.iter().map(|i| u32::from(*i)));
                    self.value(b, Op::VectorShuffle, &e.ty, &args)
                }
            }
            ExprKind::Index(base, index, count) => {
                // Computed rvalues have no declaration to address. Constant projections
                // extract directly; only dynamic array/matrix rvalues need one spill.
                let value = self.expr(b, base);
                let (index_value, guard) = self.index_guard(b, index, *count);
                if matches!(guard, Guard::Never) {
                    return self.zero(&e.ty);
                }
                if let Some(index) = access::constant_index(index) {
                    return self.value(b, Op::CompositeExtract, &e.ty, &[value, index]);
                }
                let mut path = self.owned_value(b, &base.ty, value);
                path.projections.push(Projection {
                    index: index_value,
                    constant: None,
                    ty: e.ty.clone(),
                });
                path.guard = guard;
                self.read_access(b, &path, &e.ty)
            }
            ExprKind::Unary(op, a) => {
                let a = self.expr(b, a);
                self.value(
                    b,
                    match op {
                        Unary::Not => Op::LogicalNot,
                        Unary::Complement => Op::Not,
                        Unary::Negate => {
                            if e.ty.scalar() == Some(Scalar::Float) {
                                Op::FNegate
                            } else {
                                Op::SNegate
                            }
                        }
                    },
                    &e.ty,
                    &[a],
                )
            }
            ExprKind::Binary(op, left, right) if matches!(op, Binary::And | Binary::Or) => {
                let first = self.expr(b, left);
                let condition = if *op == Binary::And {
                    first
                } else {
                    self.value(b, Op::LogicalNot, &e.ty, &[first])
                };
                let before = b.label;
                let merge = self.begin_guard(b, condition);
                let second = self.expr(b, right);
                let after = b.label;
                b.op(Op::Branch, &[merge]);
                b.op(Op::Label, &[merge]);
                self.value(b, Op::Phi, &e.ty, &[second, after, first, before])
            }
            ExprKind::Binary(op, a, c) => {
                let left = self.expr(b, a);
                let right = self.expr(b, c);
                self.binary(b, *op, &e.ty, (&a.ty, left), (&c.ty, right))
            }
            ExprKind::Construct(args) => self.construct(b, &e.ty, args),
            ExprKind::Call(i, args) => {
                let mut operands = vec![self.function_ids[*i]];
                for a in args {
                    operands.push(self.expr(b, a));
                }
                self.value(b, Op::FunctionCall, &e.ty, &operands)
            }
            ExprKind::Builtin(op, args) => self.intrinsic(b, *op, &e.ty, args),
        }
    }
    /// Scalar conversions and component packing emit exact SPIR-V constructor shapes.
    fn construct(&mut self, b: &mut Body, t: &Ty, args: &[Expr]) -> u32 {
        if matches!(t, Ty::Scalar(_)) || (args.len() == 1 && args[0].ty.lanes() == t.lanes() && matches!(t, Ty::Vector(..))) {
            let value = self.expr(b, &args[0]);
            let from = args[0].ty.scalar().unwrap();
            let to = t.scalar().unwrap();
            if from == to {
                return value;
            }
            let op = match (from, to) {
                (Scalar::Float, Scalar::Uint) => Op::ConvertFToU,
                (Scalar::Float, Scalar::Sint) => Op::ConvertFToS,
                (Scalar::Uint, Scalar::Float) => Op::ConvertUToF,
                (Scalar::Sint, Scalar::Float) => Op::ConvertSToF,
                _ => Op::Bitcast,
            };
            return self.value(b, op, t, &[value]);
        }
        let mut values = Vec::new();
        for arg in args {
            let value = self.expr(b, arg);
            if matches!(t, Ty::Vector(..)) && arg.ty.lanes() > 1 {
                for lane in 0..arg.ty.lanes() {
                    values.push(self.value(b, Op::CompositeExtract, &Ty::Scalar(arg.ty.scalar().unwrap()), &[value, u32::from(lane)]));
                }
            } else {
                values.push(value);
            }
        }
        if let Ty::Vector(_, n) = t
            && values.len() == 1
        {
            values.resize(*n as usize, values[0]);
        }
        self.value(b, Op::CompositeConstruct, t, &values)
    }
    /// Broadcasts scalars only for the scalar/vector arithmetic admitted by the frontend.
    fn splat(&mut self, b: &mut Body, value: u32, ty: &Ty) -> u32 {
        self.value(b, Op::CompositeConstruct, ty, &vec![value; ty.lanes() as usize])
    }
    /// Matrix addition/subtraction and scaling are lowered by columns; native products stay native.
    fn binary(&mut self, b: &mut Body, op: Binary, result: &Ty, left: (&Ty, u32), right: (&Ty, u32)) -> u32 {
        let (at, mut a) = left;
        let (ct, mut c) = right;
        use Binary::*;
        use Scalar::*;
        if let Ty::Matrix(cols, rows) = result {
            if op == Multiply {
                if matches!(at, Ty::Scalar(_)) {
                    return self.value(b, Op::MatrixTimesScalar, result, &[c, a]);
                }
                if matches!(ct, Ty::Scalar(_)) {
                    return self.value(b, Op::MatrixTimesScalar, result, &[a, c]);
                }
                return self.value(b, Op::MatrixTimesMatrix, result, &[a, c]);
            }
            let col = Ty::Vector(Float, *rows);
            let mut columns = Vec::new();
            for i in 0..*cols {
                let av = self.value(b, Op::CompositeExtract, &col, &[a, u32::from(i)]);
                let cv = if matches!(ct, Ty::Matrix(..)) {
                    self.value(b, Op::CompositeExtract, &col, &[c, u32::from(i)])
                } else {
                    self.splat(b, c, &col)
                };
                columns.push(self.binary(b, op, &col, (&col, av), (&col, cv)));
            }
            return self.value(b, Op::CompositeConstruct, result, &columns);
        }
        if op == Multiply && matches!(at, Ty::Matrix(..)) {
            return self.value(b, Op::MatrixTimesVector, result, &[a, c]);
        }
        if op == Multiply && matches!(ct, Ty::Matrix(..)) {
            return self.value(b, Op::VectorTimesMatrix, result, &[a, c]);
        }
        if at.lanes() != ct.lanes() {
            if at.lanes() == 1 {
                a = self.splat(b, a, ct);
            } else {
                c = self.splat(b, c, at);
            }
        }
        let family = at.scalar().unwrap();
        let opcode = match op {
            Add => {
                if family == Float {
                    Op::FAdd
                } else {
                    Op::IAdd
                }
            }
            Subtract => {
                if family == Float {
                    Op::FSub
                } else {
                    Op::ISub
                }
            }
            Multiply => {
                if family == Float {
                    Op::FMul
                } else {
                    Op::IMul
                }
            }
            Divide => match family {
                Float => Op::FDiv,
                Sint => Op::SDiv,
                _ => Op::UDiv,
            },
            Remainder => match family {
                Float => Op::FRem,
                Sint => Op::SRem,
                _ => Op::UMod,
            },
            Equal => match family {
                Float => Op::FOrdEqual,
                Bool => Op::LogicalEqual,
                _ => Op::IEqual,
            },
            NotEqual => match family {
                Float => Op::FUnordNotEqual,
                Bool => Op::LogicalNotEqual,
                _ => Op::INotEqual,
            },
            Less => match family {
                Float => Op::FOrdLessThan,
                Sint => Op::SLessThan,
                _ => Op::ULessThan,
            },
            LessEqual => match family {
                Float => Op::FOrdLessThanEqual,
                Sint => Op::SLessThanEqual,
                _ => Op::ULessThanEqual,
            },
            Greater => match family {
                Float => Op::FOrdGreaterThan,
                Sint => Op::SGreaterThan,
                _ => Op::UGreaterThan,
            },
            GreaterEqual => match family {
                Float => Op::FOrdGreaterThanEqual,
                Sint => Op::SGreaterThanEqual,
                _ => Op::UGreaterThanEqual,
            },
            And => Op::LogicalAnd,
            Or => Op::LogicalOr,
            BitAnd => Op::BitwiseAnd,
            BitOr => Op::BitwiseOr,
            Xor => Op::BitwiseXor,
            ShiftLeft => Op::ShiftLeftLogical,
            ShiftRight => {
                if family == Sint {
                    Op::ShiftRightArithmetic
                } else {
                    Op::ShiftRightLogical
                }
            }
        };
        self.value(b, opcode, result, &[a, c])
    }
    /// Emits portable intrinsics or their explicit image guards.
    fn intrinsic(&mut self, b: &mut Body, op: Builtin, t: &Ty, args: &[Expr]) -> u32 {
        use Builtin::*;
        let mut values = args.iter().map(|e| self.expr(b, e)).collect::<Vec<_>>();
        if matches!(op, Sample | SampleLevel | Load | Dimensions) {
            let i = match args[0].kind {
                ExprKind::Resource(i) => i,
                _ => unreachable!(),
            };
            let image = values[0];
            if matches!(op, Sample | SampleLevel) {
                let NativeResource::Texture { sampler, .. } = self.resources[&i] else {
                    unreachable!("validated texture operation")
                };
                let sampler = self.raw_value(b, Op::Load, self.sampler, &[sampler]);
                let sampled = self.raw_value(b, Op::SampledImage, self.sampled_image, &[image, sampler]);
                return if op == Sample {
                    self.value(b, Op::ImageSampleImplicitLod, t, &[sampled, values[1]])
                } else {
                    self.value(
                        b,
                        Op::ImageSampleExplicitLod,
                        t,
                        &[sampled, values[1], spirv::ImageOperands::LOD.bits(), values[2]],
                    )
                };
            }
            let zero = self.constant(&Ty::Scalar(Scalar::Sint), 0);
            let dimensions = self.value(b, Op::ImageQuerySizeLod, &Ty::Vector(Scalar::Uint, 2), &[image, zero]);
            if op == Dimensions {
                return dimensions;
            }
            let coord = if args[1].ty == Ty::Vector(Scalar::Uint, 2) {
                values[1]
            } else {
                self.value(b, Op::Bitcast, &Ty::Vector(Scalar::Uint, 2), &[values[1]])
            };
            let compare = self.value(b, Op::ULessThan, &Ty::Vector(Scalar::Bool, 2), &[coord, dimensions]);
            let guard = self.value(b, Op::All, &Ty::Scalar(Scalar::Bool), &[compare]);
            let empty = self.zero(t);
            let before = b.label;
            let merge = self.begin_guard(b, guard);
            let read = self.value(b, Op::ImageFetch, t, &[image, values[1], spirv::ImageOperands::LOD.bits(), zero]);
            let after = b.label;
            b.op(Op::Branch, &[merge]);
            b.op(Op::Label, &[merge]);
            return self.value(b, Op::Phi, t, &[read, after, empty, before]);
        }
        if op == Select {
            let mut cond = values[2];
            if args[2].ty.lanes() == 1 && t.lanes() > 1 {
                cond = self.splat(b, cond, &t.boolean());
            }
            return self.value(b, Op::Select, t, &[cond, values[1], values[0]]);
        }
        let native = match op {
            Dpdx => Some(Op::DPdx),
            Dpdy => Some(Op::DPdy),
            Fwidth => Some(Op::Fwidth),
            Dot => Some(Op::Dot),
            All => Some(Op::All),
            Any => Some(Op::Any),
            _ => None,
        };
        if let Some(native) = native {
            return self.value(b, native, t, &values);
        }
        let family = args[0].ty.scalar().unwrap();
        let ext = match op {
            Abs => {
                if family == Scalar::Float {
                    Glsl::FAbs
                } else {
                    Glsl::SAbs
                }
            }
            Min => match family {
                Scalar::Float => Glsl::FMin,
                Scalar::Sint => Glsl::SMin,
                _ => Glsl::UMin,
            },
            Max => match family {
                Scalar::Float => Glsl::FMax,
                Scalar::Sint => Glsl::SMax,
                _ => Glsl::UMax,
            },
            Clamp => match family {
                Scalar::Float => Glsl::FClamp,
                Scalar::Sint => Glsl::SClamp,
                _ => Glsl::UClamp,
            },
            Floor => Glsl::Floor,
            Ceil => Glsl::Ceil,
            Fract => Glsl::Fract,
            Round => Glsl::RoundEven,
            Trunc => Glsl::Trunc,
            Sqrt => Glsl::Sqrt,
            InverseSqrt => Glsl::InverseSqrt,
            Sin => Glsl::Sin,
            Cos => Glsl::Cos,
            Tan => Glsl::Tan,
            Asin => Glsl::Asin,
            Acos => Glsl::Acos,
            Atan => Glsl::Atan,
            Atan2 => Glsl::Atan2,
            Exp => Glsl::Exp,
            Exp2 => Glsl::Exp2,
            Log => Glsl::Log,
            Log2 => Glsl::Log2,
            Pow => Glsl::Pow,
            Cross => Glsl::Cross,
            Normalize => Glsl::Normalize,
            Length => Glsl::Length,
            Distance => Glsl::Distance,
            Reflect => Glsl::Reflect,
            Mix => Glsl::FMix,
            _ => unreachable!(),
        };
        let mut operands = vec![self.ext, ext as u32];
        operands.append(&mut values);
        self.value(b, Op::ExtInst, t, &operands)
    }
    /// Interface leaves are separate native variables, even when source parameters are structures.
    fn io_variable(&mut self, field: &Field, input: bool) -> u32 {
        let ty = self.ty(&field.ty);
        let variable = self.global(ty, if input { Storage::Input } else { Storage::Output });
        self.entry.push(variable);
        match field.binding.as_ref().unwrap() {
            Binding::Location(n, i, s) => {
                self.decorate(variable, Dec::Location, &[*n]);
                if *i == ShaderInterpolation::Flat && ((self.stage == GpuShaderStage::Vertex && !input) || (self.stage == GpuShaderStage::Fragment && input)) {
                    self.decorate(variable, Dec::Flat, &[]);
                }
                if *s == ShaderSampling::Centroid && ((self.stage == GpuShaderStage::Vertex && !input) || (self.stage == GpuShaderStage::Fragment && input)) {
                    self.decorate(variable, Dec::Centroid, &[]);
                }
            }
            Binding::Builtin(io) => self.decorate(
                variable,
                Dec::BuiltIn,
                &[match io {
                    BuiltinIo::Position => {
                        if input {
                            Io::FragCoord
                        } else {
                            Io::Position
                        }
                    }
                    BuiltinIo::VertexIndex => Io::VertexIndex,
                    BuiltinIo::InstanceIndex => Io::InstanceIndex,
                    BuiltinIo::FrontFacing => Io::FrontFacing,
                } as u32],
            ),
        };
        variable
    }
    /// Rebuilds a source parameter from independent native input variables.
    fn input(&mut self, b: &mut Body, field: &Field) -> u32 {
        if field.binding.is_some() {
            let pointer = self.io_variable(field, true);
            return self.value(b, Op::Load, &field.ty, &[pointer]);
        }
        let Ty::Struct(i) = field.ty else { unreachable!() };
        let values = self.m.structures[i].fields.iter().map(|f| self.input(b, f)).collect::<Vec<_>>();
        self.value(b, Op::CompositeConstruct, &field.ty, &values)
    }
    /// Extracts source results and applies canonical Vulkan clip-space Y conversion exactly once.
    fn output(&mut self, b: &mut Body, field: &Field, value: u32) {
        if field.binding.is_some() {
            let pointer = self.io_variable(field, false);
            let value = if matches!(field.binding, Some(Binding::Builtin(BuiltinIo::Position))) {
                let y = self.value(b, Op::CompositeExtract, &Ty::Scalar(Scalar::Float), &[value, 1]);
                let y = self.value(b, Op::FNegate, &Ty::Scalar(Scalar::Float), &[y]);
                self.value(b, Op::CompositeInsert, &field.ty, &[y, value, 1])
            } else {
                value
            };
            b.op(Op::Store, &[pointer, value]);
            return;
        }
        let Ty::Struct(i) = field.ty else {
            return;
        };
        for (j, f) in self.m.structures[i].fields.iter().enumerate() {
            let member = self.value(b, Op::CompositeExtract, &f.ty, &[value, j as u32]);
            self.output(b, f, member);
        }
    }
    /// Vulkan entry functions have no parameters and no return value.
    fn wrapper(&mut self, index: usize) -> u32 {
        let void = self.ty(&Ty::Void);
        let function_type = self.function_type(&[void]);
        let function = self.id();
        let label = self.id();
        let mut prefix = Vec::new();
        instruction(&mut prefix, Op::Function, &[void, function, 0, function_type]);
        instruction(&mut prefix, Op::Label, &[label]);
        let mut b = Body {
            variables: Vec::new(),
            code: Vec::new(),
            locals: Vec::new(),
            local_types: Vec::new(),
            closed: false,
            label,
            loops: Vec::new(),
        };
        let f = &self.m.functions[index];
        let mut args = vec![self.function_ids[index]];
        for p in &f.params {
            args.push(self.input(&mut b, p));
        }
        let result = self.value(&mut b, Op::FunctionCall, &f.result.ty, &args);
        self.output(&mut b, &f.result, result);
        b.op(Op::Return, &[]);
        prefix.extend(b.variables);
        prefix.extend(b.code);
        instruction(&mut prefix, Op::FunctionEnd, &[]);
        self.functions.extend(prefix);
        function
    }
}
