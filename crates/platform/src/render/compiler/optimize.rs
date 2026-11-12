//! Bounded, backend-independent expansion of aggregate helper arguments.
//!
//! Helpers cannot write resources or caller locals. Their immutable aggregate
//! parameters may therefore borrow caller storage during an expanded call rather
//! than copying a complete palette. Ordinary lets and returned snapshots keep
//! their value semantics. Validation always precedes this pass.
use super::{access, ir::*};
use std::collections::BTreeSet;

/// Limits additional IR work, independent of source size and helper-call fanout.
const EXPANSION_BUDGET: usize = 65_536;
/// Deep or large helpers retain ordinary calls rather than expanding exponentially.
const MAX_DEPTH: usize = 8;
/// Small helpers cover projection/selection utilities without duplicating large algorithms.
const MAX_HELPER_COST: usize = 512;

/// Types whose dynamic projection otherwise requires native aggregate storage.
fn aggregate(module: &Module, ty: &Ty) -> bool {
    match ty {
        Ty::Array(..) | Ty::Matrix(..) => true,
        Ty::Struct(i) => module.structures[*i].fields.iter().any(|field| aggregate(module, &field.ty)),
        _ => false,
    }
}

/// Account for each expression and optionally collect direct calls from rewritten IR.
fn expression_cost(expression: &Expr, calls: &mut BTreeSet<usize>) -> usize {
    1 + match &expression.kind {
        ExprKind::Field(base, _) | ExprKind::Swizzle(base, _) | ExprKind::Unary(_, base) => expression_cost(base, calls),
        ExprKind::Index(a, b, _) | ExprKind::Binary(_, a, b) => expression_cost(a, calls) + expression_cost(b, calls),
        ExprKind::Call(index, args) => {
            calls.insert(*index);
            args.iter().map(|arg| expression_cost(arg, calls)).sum()
        }
        ExprKind::Construct(args) | ExprKind::Builtin(_, args) => args.iter().map(|arg| expression_cost(arg, calls)).sum(),
        _ => 0,
    }
}
/// Includes control flow and declarations; the same walk refreshes native helper reachability.
fn block_cost(statements: &[Stmt], calls: &mut BTreeSet<usize>) -> usize {
    statements
        .iter()
        .map(|statement| {
            1 + match statement {
                Stmt::Let(_, value) | Stmt::Return(value) => value.as_ref().map_or(0, |value| expression_cost(value, calls)),
                Stmt::Borrow(_, value) => expression_cost(value, calls),
                Stmt::Store(target, value) => expression_cost(target, calls) + expression_cost(value, calls),
                Stmt::If(condition, yes, no) => expression_cost(condition, calls) + block_cost(yes, calls) + block_cost(no, calls),
                Stmt::While(condition, body) => expression_cost(condition, calls) + block_cost(body, calls),
                Stmt::Break | Stmt::Continue => 0,
            }
        })
        .sum()
}

/// Whole-module admission metadata remains authoritative. Only call reachability
/// changes; original reflection and derivative/stage validation are preserved.
pub(super) fn run(module: &mut Module) {
    let originals = std::mem::take(&mut module.functions);
    let costs: Vec<_> = originals
        .iter()
        .map(|function| block_cost(&function.body, &mut BTreeSet::new()) + function.locals.len() * 4 + 8)
        .collect();
    let mut remaining = EXPANSION_BUDGET;
    let mut functions = Vec::with_capacity(originals.len());
    for (index, original) in originals.iter().enumerate() {
        let mut rewrite = Rewrite {
            module,
            originals: &originals,
            costs: &costs,
            locals: original.locals.clone(),
            remaining: &mut remaining,
            depth: 0,
        };
        let ids: Vec<_> = (0..original.locals.len()).collect();
        let scope = Scope {
            ids: &ids,
            function: index,
            exit: None,
            expanded: false,
        };
        let body = rewrite.block(&original.body, &scope);
        let mut function = original.clone();
        function.locals = rewrite.locals;
        function.body = body;
        function.calls.clear();
        block_cost(&function.body, &mut function.calls);
        functions.push(function);
    }
    module.functions = functions;
}

/// Expanded returns write a result and terminate the remaining callee work via a flag.
#[derive(Clone, Copy)]
struct Exit {
    /// Mutable boolean local initialized false at the call site.
    returned: usize,
    /// Mutable result local, initialized before any conditional return.
    result: usize,
}
/// Source-local indices map to fresh caller indices; only an expanded call has an exit flag.
struct Scope<'a> {
    /// Complete local-index map for this source body.
    ids: &'a [usize],
    /// Original function supplying mutability and parameter declarations.
    function: usize,
    /// None for a native function or straight-line expansion.
    exit: Option<Exit>,
    /// Immutable aliases of callee parameters may borrow for the call's duration.
    expanded: bool,
}
/// One function owns its rewritten local table; all source bodies are immutable borrows.
struct Rewrite<'a> {
    /// Structural type declarations used to classify aggregate parameters.
    module: &'a Module,
    /// Validated source functions, never partially overwritten during expansion.
    originals: &'a [Function],
    /// Finite per-helper expansion charges computed once.
    costs: &'a [usize],
    /// Original and generated caller locals, each with one concrete type and ownership mode.
    locals: Vec<Local>,
    /// Shared whole-module budget; exhaustion preserves an ordinary call.
    remaining: &'a mut usize,
    /// Current expansion depth, independent of the source call graph's size.
    depth: usize,
}
/// Typed local reference; generated names are diagnostic-only, writers use indices.
fn local(index: usize, ty: &Ty) -> Expr {
    Expr {
        ty: ty.clone(),
        kind: ExprKind::Local(index),
    }
}
/// Boolean control constants never require host floating-point folding.
fn boolean(value: bool) -> Expr {
    Expr {
        ty: Ty::Scalar(Scalar::Bool),
        kind: ExprKind::Literal(Literal::Bool(value)),
    }
}
/// Negation retains scalar-bool typing established for generated control locals.
fn not(value: Expr) -> Expr {
    Expr {
        ty: value.ty.clone(),
        kind: ExprKind::Unary(Unary::Not, Box::new(value)),
    }
}
/// A source statement may exit from any nested branch or loop.
fn returns(statement: &Stmt) -> bool {
    match statement {
        Stmt::Return(_) => true,
        Stmt::If(_, yes, no) => yes.iter().chain(no).any(returns),
        Stmt::While(_, body) => body.iter().any(returns),
        _ => false,
    }
}

impl Rewrite<'_> {
    /// Generated locals use fresh indices; helper lexical names cannot collide with the caller.
    fn new_local(&mut self, ty: Ty, mutable: bool) -> usize {
        let index = self.locals.len();
        self.locals.push(Local {
            name: format!("inline{index}"),
            ty,
            mutable,
        });
        index
    }
    /// Preserve left-to-right operand evaluation when a later operand expands into statements.
    fn operands(&mut self, values: &[Expr], scope: &Scope<'_>) -> (Vec<Stmt>, Vec<Expr>) {
        let lowered: Vec<_> = values.iter().map(|value| self.expression(value, scope)).collect();
        let mut before = Vec::new();
        let mut expressions = Vec::with_capacity(values.len());
        let last_prefix = lowered.iter().rposition(|(prefix, _)| !prefix.is_empty());
        for (i, (prefix, mut value)) in lowered.into_iter().enumerate() {
            before.extend(prefix);
            if last_prefix.is_some_and(|last| i < last) && !matches!(value.kind, ExprKind::Literal(_)) {
                let id = self.new_local(value.ty.clone(), false);
                before.push(if aggregate(self.module, &value.ty) {
                    Stmt::Borrow(id, value.clone())
                } else {
                    Stmt::Let(id, Some(value.clone()))
                });
                value = local(id, &value.ty);
            }
            expressions.push(value);
        }
        (before, expressions)
    }
    /// Each expression returns the statements that must execute immediately before it.
    fn expression(&mut self, value: &Expr, scope: &Scope<'_>) -> (Vec<Stmt>, Expr) {
        let mut prefix = Vec::new();
        let kind = match &value.kind {
            ExprKind::Literal(value) => ExprKind::Literal(value.clone()),
            ExprKind::Local(index) => ExprKind::Local(scope.ids[*index]),
            ExprKind::Resource(index) => ExprKind::Resource(*index),
            ExprKind::Field(base, index) => {
                let (before, base) = self.expression(base, scope);
                prefix = before;
                ExprKind::Field(Box::new(base), *index)
            }
            ExprKind::Swizzle(base, lanes) => {
                let (before, base) = self.expression(base, scope);
                prefix = before;
                ExprKind::Swizzle(Box::new(base), lanes.clone())
            }
            ExprKind::Unary(op, base) => {
                let (before, base) = self.expression(base, scope);
                prefix = before;
                ExprKind::Unary(*op, Box::new(base))
            }
            ExprKind::Index(base, index, count) => {
                let (before, values) = self.operands(&[(**base).clone(), (**index).clone()], scope);
                prefix = before;
                ExprKind::Index(Box::new(values[0].clone()), Box::new(values[1].clone()), *count)
            }
            ExprKind::Binary(op, left, right) if matches!(op, Binary::And | Binary::Or) => {
                let (before, left) = self.expression(left, scope);
                prefix = before;
                let (mut conditional, right) = self.expression(right, scope);
                if conditional.is_empty() {
                    ExprKind::Binary(*op, Box::new(left), Box::new(right))
                } else {
                    let id = self.new_local(value.ty.clone(), true);
                    prefix.push(Stmt::Let(id, Some(left)));
                    let result = local(id, &value.ty);
                    conditional.push(Stmt::Store(result.clone(), right));
                    prefix.push(Stmt::If(
                        if *op == Binary::And { result.clone() } else { not(result.clone()) },
                        conditional,
                        Vec::new(),
                    ));
                    return (prefix, result);
                }
            }
            ExprKind::Binary(op, left, right) => {
                let (before, values) = self.operands(&[(**left).clone(), (**right).clone()], scope);
                prefix = before;
                ExprKind::Binary(*op, Box::new(values[0].clone()), Box::new(values[1].clone()))
            }
            ExprKind::Construct(args) => {
                let (before, args) = self.operands(args, scope);
                prefix = before;
                ExprKind::Construct(args)
            }
            ExprKind::Builtin(op, args) => {
                let (before, args) = self.operands(args, scope);
                prefix = before;
                ExprKind::Builtin(*op, args)
            }
            ExprKind::Call(index, args) => {
                let (before, args) = self.operands(args, scope);
                prefix = before;
                let function = &self.originals[*index];
                let cost = self.costs[*index];
                if function.stage.is_none()
                    && function.result.ty != Ty::Void
                    && self.depth < MAX_DEPTH
                    && cost <= MAX_HELPER_COST
                    && cost <= *self.remaining
                    && function.params.iter().any(|parameter| aggregate(self.module, &parameter.ty))
                {
                    *self.remaining -= cost;
                    let (body, result) = self.expand(*index, args);
                    prefix.extend(body);
                    return (prefix, result);
                }
                ExprKind::Call(*index, args)
            }
        };
        (prefix, Expr { ty: value.ty.clone(), kind })
    }
    /// Aggregate parameters borrow only while the caller is suspended. Scalar
    /// arguments still capture eagerly, including unused derivative/sample results.
    fn expand(&mut self, index: usize, args: Vec<Expr>) -> (Vec<Stmt>, Expr) {
        let function = &self.originals[index];
        let ids: Vec<_> = function.locals.iter().map(|source| self.new_local(source.ty.clone(), source.mutable)).collect();
        let mut body = Vec::new();
        for (i, value) in args.into_iter().enumerate() {
            body.push(if aggregate(self.module, &value.ty) {
                Stmt::Borrow(ids[i], value)
            } else {
                Stmt::Let(ids[i], Some(value))
            });
        }
        self.depth += 1;
        let straight = matches!(function.body.last(), Some(Stmt::Return(Some(_))))
            && function.body[..function.body.len() - 1]
                .iter()
                .all(|statement| matches!(statement, Stmt::Let(..) | Stmt::Store(..)));
        let result = if straight {
            let scope = Scope {
                ids: &ids,
                function: index,
                exit: None,
                expanded: true,
            };
            body.extend(self.block(&function.body[..function.body.len() - 1], &scope));
            let Stmt::Return(Some(value)) = function.body.last().unwrap() else {
                unreachable!()
            };
            let (before, value) = self.expression(value, &scope);
            body.extend(before);
            value
        } else {
            let returned = self.new_local(Ty::Scalar(Scalar::Bool), true);
            let result = self.new_local(function.result.ty.clone(), true);
            body.push(Stmt::Let(returned, Some(boolean(false))));
            body.push(Stmt::Let(result, None));
            let scope = Scope {
                ids: &ids,
                function: index,
                exit: Some(Exit { returned, result }),
                expanded: true,
            };
            body.extend(self.block(&function.body, &scope));
            local(result, &function.result.ty)
        };
        self.depth -= 1;
        (body, result)
    }
    /// Lvalue roots must remain the original mutable declaration. Only index
    /// expressions expand; capturing an aggregate base would redirect the write.
    fn lvalue(&mut self, value: &Expr, scope: &Scope<'_>) -> (Vec<Stmt>, Expr) {
        let (before, kind) = match &value.kind {
            ExprKind::Local(i) => (Vec::new(), ExprKind::Local(scope.ids[*i])),
            ExprKind::Field(base, i) => {
                let (before, base) = self.lvalue(base, scope);
                (before, ExprKind::Field(Box::new(base), *i))
            }
            ExprKind::Swizzle(base, lanes) => {
                let (before, base) = self.lvalue(base, scope);
                (before, ExprKind::Swizzle(Box::new(base), lanes.clone()))
            }
            ExprKind::Index(base, index, count) => {
                let (mut before, base) = self.lvalue(base, scope);
                let (prefix, index) = self.expression(index, scope);
                before.extend(prefix);
                (before, ExprKind::Index(Box::new(base), Box::new(index), *count))
            }
            _ => unreachable!("validated mutable declaration path"),
        };
        (before, Expr { ty: value.ty.clone(), kind })
    }
    /// Keep lexical suffixes together after an early return. Loop conditions and
    /// their expanded calls remain inside the loop, including across continue.
    fn block(&mut self, statements: &[Stmt], scope: &Scope<'_>) -> Vec<Stmt> {
        let mut output = Vec::new();
        for (position, statement) in statements.iter().enumerate() {
            match statement {
                Stmt::Let(index, value) => {
                    let value = value.as_ref().map(|value| {
                        let (prefix, value) = self.expression(value, scope);
                        output.extend(prefix);
                        value
                    });
                    let borrow = scope.expanded
                        && value.is_some()
                        && match &statements[position] {
                            Stmt::Let(_, Some(original)) => access::path(original).is_some_and(|path| match path.root {
                                access::Root::Uniform(_) => true,
                                access::Root::Local(i) => !self.originals[scope.function].locals[i].mutable,
                            }),
                            _ => false,
                        }
                        && !self.originals[scope.function].locals[*index].mutable;
                    output.push(if borrow {
                        Stmt::Borrow(scope.ids[*index], value.unwrap())
                    } else {
                        Stmt::Let(scope.ids[*index], value)
                    });
                }
                Stmt::Borrow(_, _) => unreachable!("source validation never creates optimizer views"),
                Stmt::Store(target, value) => {
                    let (before, value) = self.expression(value, scope);
                    output.extend(before);
                    // The RHS must finish before any writable-path index helper runs.
                    let (before, target) = self.lvalue(target, scope);
                    output.extend(before);
                    output.push(Stmt::Store(target, value));
                }
                Stmt::Return(value) => {
                    let value = value.as_ref().map(|value| {
                        let (prefix, value) = self.expression(value, scope);
                        output.extend(prefix);
                        value
                    });
                    if let Some(exit) = scope.exit {
                        let value = value.expect("expanded helpers return a value");
                        output.push(Stmt::Store(local(exit.result, &value.ty), value));
                        output.push(Stmt::Store(local(exit.returned, &Ty::Scalar(Scalar::Bool)), boolean(true)));
                    } else {
                        output.push(Stmt::Return(value));
                    }
                }
                Stmt::If(condition, yes, no) => {
                    let (before, condition) = self.expression(condition, scope);
                    output.extend(before);
                    output.push(Stmt::If(condition, self.block(yes, scope), self.block(no, scope)));
                }
                Stmt::While(condition, body) => {
                    let (mut before, condition) = self.expression(condition, scope);
                    if let Some(exit) = scope.exit {
                        before.insert(0, Stmt::If(local(exit.returned, &Ty::Scalar(Scalar::Bool)), vec![Stmt::Break], Vec::new()));
                    }
                    before.push(Stmt::If(not(condition), vec![Stmt::Break], Vec::new()));
                    before.extend(self.block(body, scope));
                    output.push(Stmt::While(boolean(true), before));
                }
                Stmt::Break => output.push(Stmt::Break),
                Stmt::Continue => output.push(Stmt::Continue),
            }
            if let Some(exit) = scope.exit
                && returns(statement)
            {
                let suffix = &statements[position + 1..];
                if !suffix.is_empty() {
                    output.push(Stmt::If(
                        not(local(exit.returned, &Ty::Scalar(Scalar::Bool))),
                        self.block(suffix, scope),
                        Vec::new(),
                    ));
                }
                break;
            }
        }
        output
    }
}
