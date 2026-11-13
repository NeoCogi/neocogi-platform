//! Small Python-like frontend lowering directly into the shared source-positioned tree.
//! This is a statically typed shader language, without Python execution or imports.
mod lex;
mod locals;
use super::{
    Diagnostic,
    syntax::{Node, Value},
};
use lex::{Kind, Token};
use std::collections::BTreeSet;

/// Parse both handwritten and generated syntax under the shared tree budget.
pub(super) fn parse(source: &str) -> Result<Node<'_>, Diagnostic> {
    let tokens = lex::tokens(source)?;
    let names = tokens
        .iter()
        .filter_map(|token| if let Kind::Word(name) = token.kind { Some(name) } else { None })
        .collect();
    let mut reader = Parser {
        tokens,
        cursor: 0,
        depth: 0,
        names,
        temporary: 0,
    };
    let root = reader.module()?;
    let mut pending = vec![&root];
    let mut count = 0;
    while let Some(node) = pending.pop() {
        count += 1;
        if count > 100_000 {
            return Err(node.error("syntax complexity limit exceeded"));
        }
        if let Value::List(children) = &node.value {
            pending.extend(children);
        }
    }
    Ok(root)
}
/// A complete frontend owns its tokens and fresh-name counter; all ordinary atoms borrow source.
struct Parser<'a> {
    /// Bounded lexical stream, ending in one explicit End token.
    tokens: Vec<Token<'a>>,
    /// Next token, never beyond the End token.
    cursor: usize,
    /// Recursive expression/control parsing depth, independent of bracket/indent limits.
    depth: usize,
    /// Every written word, used to avoid collisions with compiler-generated locals.
    names: BTreeSet<&'a str>,
    /// Monotonic fresh-local sequence within this module.
    temporary: usize,
}
/// Internal forms own normalized operators while preserving operand and original byte positions.
fn form<'a>(offset: usize, head: &'a str, mut tail: Vec<Node<'a>>) -> Result<Node<'a>, Diagnostic> {
    tail.insert(0, Node::word(offset, head));
    Node::form(offset, tail)
}
/// Reserved Python constructs reject rather than becoming surprising shader variable names.
fn reserved(word: &str) -> bool {
    matches!(
        word,
        "def"
            | "class"
            | "if"
            | "elif"
            | "else"
            | "while"
            | "for"
            | "in"
            | "is"
            | "and"
            | "or"
            | "not"
            | "return"
            | "break"
            | "continue"
            | "pass"
            | "True"
            | "False"
            | "None"
            | "import"
            | "from"
            | "as"
            | "lambda"
            | "with"
            | "async"
            | "await"
            | "try"
            | "except"
            | "finally"
            | "raise"
            | "yield"
            | "global"
            | "nonlocal"
            | "del"
            | "assert"
            | "match"
            | "case"
            | "array"
            | "true"
            | "false"
    )
}
/// Source assignment targets must remain syntactic local paths, even if an
/// identity operator or helper later lowers to an addressable typed value.
fn writable_path(node: &Node<'_>) -> bool {
    if let Ok(name) = node.atom() {
        return !reserved(name)
            && name.as_bytes().first().is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    }
    let Ok(values) = node.list() else {
        return false;
    };
    values.len() == 3 && matches!(values[0].atom().ok(), Some("." | "index")) && writable_path(&values[1])
}
/// Python precedence, with right-associative power above unary signs.
fn infix(kind: Kind<'_>) -> Option<(&'static str, u8, u8)> {
    Some(match kind {
        Kind::Word("or") => ("||", 1, 2),
        Kind::Word("and") => ("&&", 3, 4),
        Kind::Symbol("==") => ("==", 7, 8),
        Kind::Symbol("!=") => ("!=", 7, 8),
        Kind::Symbol("<") => ("<", 7, 8),
        Kind::Symbol("<=") => ("<=", 7, 8),
        Kind::Symbol(">") => (">", 7, 8),
        Kind::Symbol(">=") => (">=", 7, 8),
        Kind::Symbol("|") => ("|", 9, 10),
        Kind::Symbol("^") => ("^", 11, 12),
        Kind::Symbol("&") => ("&", 13, 14),
        Kind::Symbol("<<") => ("<<", 15, 16),
        Kind::Symbol(">>") => (">>", 15, 16),
        Kind::Symbol("+") => ("+", 17, 18),
        Kind::Symbol("-") => ("-", 17, 18),
        Kind::Symbol("*") => ("*", 19, 20),
        Kind::Symbol("/") => ("/", 19, 20),
        Kind::Symbol("%") => ("%", 19, 20),
        Kind::Symbol("@") => ("@", 19, 20),
        Kind::Symbol("**") => ("pow", 23, 22),
        _ => return None,
    })
}
impl<'a> Parser<'a> {
    /// The End token permits diagnostics at EOF without sentinel source offsets.
    fn token(&self) -> Token<'a> {
        self.tokens[self.cursor]
    }
    /// Preserve a precise offending-token location for all grammar failures.
    fn error(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            offset: self.token().offset,
            message: message.into(),
        }
    }
    /// Consume one token; no successful parser advances beyond End.
    fn next(&mut self) -> Token<'a> {
        let token = self.token();
        if token.kind != Kind::End {
            self.cursor += 1;
        }
        token
    }
    /// Optional structural or spelling match.
    fn take(&mut self, kind: Kind<'a>) -> bool {
        if self.token().kind == kind {
            self.next();
            true
        } else {
            false
        }
    }
    /// Required spelling/structure with the original token's error location.
    fn expect(&mut self, kind: Kind<'a>, message: &str) -> Result<(), Diagnostic> {
        if self.take(kind) { Ok(()) } else { Err(self.error(message)) }
    }
    /// Blank physical lines are already ignored; tolerate the final synthetic newline too.
    fn lines(&mut self) {
        while self.take(Kind::Newline) {}
    }
    /// An ordinary source identifier; semantic resolution still owns length and collision checks.
    fn identifier(&mut self) -> Result<Node<'a>, Diagnostic> {
        let Token { offset, kind: Kind::Word(word) } = self.token() else {
            return Err(self.error("expected an identifier"));
        };
        if reserved(word) {
            return Err(self.error(format!("'{word}' is reserved in Python-like shader syntax")));
        }
        self.next();
        Ok(Node::word(offset, word))
    }
    /// Attribute arguments are names or nonnegative decimal indices, never expressions.
    fn attribute_argument(&mut self) -> Result<Node<'a>, Diagnostic> {
        let token = self.next();
        match token.kind {
            Kind::Word(word) | Kind::Number(word) => Ok(Node::word(token.offset, word)),
            _ => Err(Diagnostic {
                offset: token.offset,
                message: "expected an attribute name or index".into(),
            }),
        }
    }
    /// Value types use array[TYPE, COUNT]; shader IO attributes are handled separately.
    fn ty(&mut self) -> Result<Node<'a>, Diagnostic> {
        let token = self.next();
        let Kind::Word(word) = token.kind else {
            return Err(Diagnostic {
                offset: token.offset,
                message: "expected a shader type".into(),
            });
        };
        if word == "None" {
            return Ok(Node::word(token.offset, "void"));
        }
        if word != "array" {
            return Ok(Node::word(token.offset, word));
        }
        self.expect(Kind::Symbol("["), "expected '[' after array")?;
        let base = self.ty()?;
        self.expect(Kind::Symbol(","), "expected ',' before array length")?;
        let count = self.attribute_argument()?;
        self.expect(Kind::Symbol("]"), "expected ']' after array type")?;
        form(token.offset, "array", vec![base, count])
    }
    /// builtin[T, NAME] and location[T, N, MODE?, SAMPLING?] are Python-style annotations.
    fn annotation(&mut self) -> Result<Vec<Node<'a>>, Diagnostic> {
        let token = self.token();
        let wrapper = match token.kind {
            Kind::Word("builtin") => Some("builtin"),
            Kind::Word("location") => Some("location"),
            _ => None,
        };
        let Some(wrapper) = wrapper else {
            return Ok(vec![self.ty()?]);
        };
        self.next();
        self.expect(Kind::Symbol("["), "expected '[' in IO annotation")?;
        let ty = self.ty()?;
        self.expect(Kind::Symbol(","), "expected ',' before IO binding")?;
        let binding = self.attribute_argument()?;
        let mut fields = vec![ty, form(token.offset, wrapper, vec![binding])?];
        if wrapper == "location" && self.take(Kind::Symbol(",")) {
            let mode = self.attribute_argument()?;
            let mut values = vec![mode];
            if self.take(Kind::Symbol(",")) {
                values.push(self.attribute_argument()?);
            }
            fields.push(form(token.offset, "interpolate", values)?);
        }
        self.expect(Kind::Symbol("]"), "expected ']' after IO annotation")?;
        Ok(fields)
    }
    /// Every module declaration lowers to the same semantic tree as S-expressions.
    fn module(&mut self) -> Result<Node<'a>, Diagnostic> {
        let mut declarations = vec![Node::word(0, "module")];
        self.lines();
        while self.token().kind != Kind::End {
            let declaration = match self.token().kind {
                Kind::Word("class") => self.structure()?,
                Kind::Word("def") => self.function("fn")?,
                Kind::Symbol("@") => {
                    self.next();
                    let stage = match self.next() {
                        Token { kind: Kind::Word("vertex"), .. } => "vertex",
                        Token { kind: Kind::Word("fragment"), .. } => "fragment",
                        token => {
                            return Err(Diagnostic {
                                offset: token.offset,
                                message: "entry decorator must be @vertex or @fragment".into(),
                            });
                        }
                    };
                    self.expect(Kind::Newline, "expected a newline after entry decorator")?;
                    self.lines();
                    self.function(stage)?
                }
                Kind::Word(_) => self.resource()?,
                _ => return Err(self.error("expected class, def, entry decorator, or typed resource binding")),
            };
            declarations.push(declaration);
            self.lines();
        }
        Node::form(0, declarations)
    }
    /// Suites require a physical newline and consistent space indentation, without single-line bodies.
    fn begin_suite(&mut self) -> Result<(), Diagnostic> {
        self.expect(Kind::Symbol(":"), "expected ':' before indented body")?;
        self.expect(Kind::Newline, "expected a newline before indented body")?;
        self.lines();
        self.expect(Kind::Indent, "expected an indented body")
    }
    /// Structures have only typed fields; inheritance and methods are deliberately absent.
    fn structure(&mut self) -> Result<Node<'a>, Diagnostic> {
        let offset = self.next().offset;
        let name = self.identifier()?;
        self.begin_suite()?;
        let mut fields = vec![name];
        self.lines();
        while !matches!(self.token().kind, Kind::Dedent | Kind::End) {
            let name = self.identifier()?;
            self.expect(Kind::Symbol(":"), "expected ':' after field name")?;
            let mut values = vec![name];
            values.extend(self.annotation()?);
            fields.push(Node::form(values[0].offset, values)?);
            self.expect(Kind::Newline, "expected a newline after field annotation")?;
            self.lines();
        }
        self.expect(Kind::Dedent, "expected end of structure body")?;
        form(offset, "struct", fields)
    }
    /// Module values can only be read-only, explicitly staged uniform/texture bindings.
    fn resource(&mut self) -> Result<Node<'a>, Diagnostic> {
        let name = self.identifier()?;
        self.expect(Kind::Symbol(":"), "expected ':' after resource name; mutable globals are unsupported")?;
        let token = self.next();
        let kind = match token.kind {
            Kind::Word("uniform") => "uniform",
            Kind::Word("texture") => "texture",
            _ => {
                return Err(Diagnostic {
                    offset: token.offset,
                    message: "resource annotation must be uniform[TYPE, STAGE, SLOT] or texture[STAGE, SLOT]".into(),
                });
            }
        };
        self.expect(Kind::Symbol("["), "expected '[' in resource annotation")?;
        let mut values = vec![name];
        if kind == "uniform" {
            values.push(self.ty()?);
            self.expect(Kind::Symbol(","), "expected ',' after uniform type")?;
        }
        values.push(self.attribute_argument()?);
        self.expect(Kind::Symbol(","), "expected ',' before resource slot")?;
        values.push(self.attribute_argument()?);
        self.expect(Kind::Symbol("]"), "expected ']' after resource annotation")?;
        self.expect(Kind::Newline, "expected a newline after resource annotation")?;
        form(values[0].offset, kind, values)
    }
    /// Signatures require explicit parameter types; an omitted return type means None/void.
    fn function(&mut self, stage: &'static str) -> Result<Node<'a>, Diagnostic> {
        let offset = self.token().offset;
        self.expect(Kind::Word("def"), "entry decorator must be followed by def")?;
        let name = self.identifier()?;
        self.expect(Kind::Symbol("("), "expected '(' after function name")?;
        let mut parameters = Vec::new();
        while !self.take(Kind::Symbol(")")) {
            let name = self.identifier()?;
            self.expect(Kind::Symbol(":"), "function parameters require a type annotation")?;
            let mut field = vec![name];
            field.extend(self.annotation()?);
            parameters.push(Node::form(field[0].offset, field)?);
            if self.take(Kind::Symbol(")")) {
                break;
            }
            self.expect(Kind::Symbol(","), "expected ',' between parameters")?;
        }
        let result = if self.take(Kind::Symbol("->")) {
            self.annotation()?
        } else {
            vec![Node::word(offset, "void")]
        };
        let mut body = self.suite()?;
        locals::resolve(&mut body, &parameters)?;
        let mut values = vec![name, Node::form(offset, parameters)?, form(offset, "returns", result)?];
        values.extend(body);
        form(offset, stage, values)
    }
    /// An indented lexical block ends at its own dedent, never consuming its parent's newline.
    fn suite(&mut self) -> Result<Vec<Node<'a>>, Diagnostic> {
        self.begin_suite()?;
        let mut statements = Vec::new();
        self.lines();
        while !matches!(self.token().kind, Kind::Dedent | Kind::End) {
            statements.extend(self.statement()?);
            self.lines();
        }
        self.expect(Kind::Dedent, "expected end of indented body")?;
        Ok(statements)
    }
    /// Both elif chains and nested statement/expression recursion consume this finite allowance.
    fn statement(&mut self) -> Result<Vec<Node<'a>>, Diagnostic> {
        self.depth += 1;
        if self.depth > 96 {
            return Err(self.error("syntax complexity limit exceeded"));
        }
        let result = self.statement_inner();
        self.depth -= 1;
        result
    }
    /// Conditions remain statements; assignment is never accepted as an expression.
    fn statement_inner(&mut self) -> Result<Vec<Node<'a>>, Diagnostic> {
        let token = self.token();
        match token.kind {
            Kind::Word("if") => Ok(vec![self.conditional()?]),
            Kind::Word("while") => {
                self.next();
                let mut values = vec![self.expression(0)?];
                values.extend(self.suite()?);
                Ok(vec![form(token.offset, "while", values)?])
            }
            Kind::Word("return" | "break" | "continue" | "pass") => {
                self.next();
                let Kind::Word(word) = token.kind else { unreachable!() };
                let values = if word == "return" && self.take(Kind::Word("None")) || self.token().kind == Kind::Newline {
                    Vec::new()
                } else {
                    vec![self.expression(0)?]
                };
                self.expect(Kind::Newline, "expected a newline after statement")?;
                Ok(vec![form(token.offset, if word == "pass" { "$pass" } else { word }, values)?])
            }
            _ => {
                let target = self.expression(0)?;
                if !writable_path(&target) {
                    return Err(target.error("assignment requires a local name or a writable member/index path"));
                }
                if self.take(Kind::Symbol(":")) {
                    target.atom().map_err(|_| target.error("only a local name can have a type annotation"))?;
                    let mut values = vec![target, self.ty()?];
                    if self.take(Kind::Symbol("=")) {
                        values.push(self.expression(0)?);
                    }
                    self.expect(Kind::Newline, "expected a newline after local declaration")?;
                    return Ok(vec![form(token.offset, "$declare", values)?]);
                }
                let assignment = self.next();
                let Kind::Symbol(operator) = assignment.kind else {
                    return Err(Diagnostic {
                        offset: assignment.offset,
                        message: "expected '=' or an augmented assignment".into(),
                    });
                };
                if operator == "=" {
                    let value = self.expression(0)?;
                    self.expect(Kind::Newline, "expected a newline after assignment")?;
                    return Ok(vec![form(token.offset, "$assign", vec![target, value])?]);
                }
                let Some(operator) = operator.strip_suffix('=').and_then(|op| infix(Kind::Symbol(op))) else {
                    return Err(Diagnostic {
                        offset: assignment.offset,
                        message: "expected '=' or an augmented assignment".into(),
                    });
                };
                // Augmented assignment evaluates the target's indices and old value
                // exactly once, before evaluating its right operand.
                let mut prefix = Vec::new();
                let target = self.capture_indices(target, &mut prefix)?;
                let old = self.fresh(token.offset);
                prefix.push(form(token.offset, "let", vec![old.clone(), target.clone()])?);
                let value = self.expression(0)?;
                self.expect(Kind::Newline, "expected a newline after augmented assignment")?;
                prefix.push(form(
                    token.offset,
                    "$assign",
                    vec![target, form(assignment.offset, operator.0, vec![old, value])?],
                )?);
                Ok(prefix)
            }
        }
    }
    /// Only an existing if-chain may consume elif. Recursion is bounded even
    /// when all clauses have identical indentation rather than deeper suites.
    fn conditional(&mut self) -> Result<Node<'a>, Diagnostic> {
        self.depth += 1;
        if self.depth > 96 {
            return Err(self.error("syntax complexity limit exceeded"));
        }
        let result = self.conditional_inner();
        self.depth -= 1;
        result
    }
    /// Translate elif to a nested conditional in its parent's else scope.
    fn conditional_inner(&mut self) -> Result<Node<'a>, Diagnostic> {
        let offset = self.next().offset;
        let condition = self.expression(0)?;
        let yes = form(offset, "then", self.suite()?)?;
        let mut values = vec![condition, yes];
        if self.take(Kind::Word("else")) {
            values.push(form(offset, "else", self.suite()?)?);
        } else if self.token().kind == Kind::Word("elif") {
            let no = self.conditional()?;
            values.push(form(offset, "else", vec![no])?);
        }
        form(offset, "if", values)
    }
    /// Names are fresh against every written word, including later declarations and other scopes.
    fn fresh(&mut self, offset: usize) -> Node<'a> {
        loop {
            let name = format!("__shader{}", self.temporary);
            self.temporary += 1;
            if !self.names.contains(name.as_str()) {
                return Node::word(offset, name);
            }
        }
    }
    /// Capture only writable-path indices, preserving the original root and projection order.
    fn capture_indices(&mut self, mut target: Node<'a>, prefix: &mut Vec<Node<'a>>) -> Result<Node<'a>, Diagnostic> {
        if let Value::List(values) = &mut target.value {
            let index = values.first().and_then(|node| node.atom().ok()) == Some("index");
            let field = values.first().and_then(|node| node.atom().ok()) == Some(".");
            if (index || field) && values.len() == 3 {
                values[1] = self.capture_indices(values[1].clone(), prefix)?;
                if index {
                    let index = self.fresh(values[2].offset);
                    prefix.push(form(index.offset, "let", vec![index.clone(), values[2].clone()])?);
                    values[2] = index;
                }
            }
        }
        Ok(target)
    }
    /// Recursive Pratt entry guards prefix/power/call nesting before building owned trees.
    fn expression(&mut self, minimum: u8) -> Result<Node<'a>, Diagnostic> {
        self.depth += 1;
        if self.depth > 96 {
            return Err(self.error("syntax complexity limit exceeded"));
        }
        let result = self.expression_inner(minimum);
        self.depth -= 1;
        result
    }
    /// Postfix projection binds most tightly. Chained comparisons reject rather than
    /// silently acquiring non-Python evaluation semantics.
    fn expression_inner(&mut self, minimum: u8) -> Result<Node<'a>, Diagnostic> {
        let token = self.next();
        let mut left = match token.kind {
            Kind::Number(value) => Node::word(token.offset, value),
            Kind::Word("True") => Node::word(token.offset, "true"),
            Kind::Word("False") => Node::word(token.offset, "false"),
            Kind::Word("not") if minimum <= 5 => form(token.offset, "!", vec![self.expression(5)?])?,
            Kind::Word("array") if self.token().kind == Kind::Symbol("[") => {
                self.cursor -= 1;
                self.ty()?
            }
            Kind::Word(word) if !reserved(word) => Node::word(token.offset, word),
            Kind::Symbol("+" | "-" | "~") => {
                let Kind::Symbol(operator) = token.kind else { unreachable!() };
                // Keep signed literals (including i32::MIN) representable, while
                // preserving Python's higher power precedence: -x**y == -(x**y).
                if let Kind::Number(number) = self.token().kind
                    && self.tokens.get(self.cursor + 1).is_none_or(|token| token.kind != Kind::Symbol("**"))
                    && operator != "~"
                {
                    self.next();
                    Node::word(token.offset, format!("{operator}{number}"))
                } else {
                    let value = self.expression(21)?;
                    form(token.offset, if operator == "+" { "$plus" } else { operator }, vec![value])?
                }
            }
            Kind::Symbol("(") => {
                let value = self.expression(0)?;
                self.expect(Kind::Symbol(")"), "expected ')' after grouped expression")?;
                value
            }
            Kind::Symbol("[") => form(token.offset, "$array", self.arguments("]")?)?,
            _ => {
                return Err(Diagnostic {
                    offset: token.offset,
                    message: "expected a shader expression".into(),
                });
            }
        };
        let mut comparison = false;
        loop {
            match self.token().kind {
                Kind::Symbol(".") => {
                    let offset = self.next().offset;
                    let member = self.identifier()?;
                    left = form(offset, ".", vec![left, member])?;
                    continue;
                }
                Kind::Symbol("[") => {
                    let offset = self.next().offset;
                    let index = self.expression(0)?;
                    self.expect(Kind::Symbol("]"), "expected ']' after index")?;
                    left = form(offset, "index", vec![left, index])?;
                    continue;
                }
                Kind::Symbol("(") => {
                    let offset = left.offset;
                    self.next();
                    let mut arguments = self.arguments(")")?;
                    if left.atom().ok() == Some("u32")
                        && arguments.len() == 1
                        && let Ok(number) = arguments[0].atom()
                        && !number.is_empty()
                        && number.bytes().all(|b| b.is_ascii_digit())
                    {
                        number.parse::<u32>().map_err(|_| arguments[0].error("invalid u32 literal"))?;
                        left = Node::word(offset, format!("{number}u"));
                        continue;
                    }
                    arguments.insert(0, left);
                    left = form(offset, "$call", arguments)?;
                    continue;
                }
                _ => {}
            }
            let Some((operator, l, r)) = infix(self.token().kind) else {
                break;
            };
            if l < minimum {
                break;
            }
            let offset = self.next().offset;
            if l == 7 {
                if comparison {
                    return Err(Diagnostic {
                        offset,
                        message: "chained comparisons are unsupported; combine comparisons with and".into(),
                    });
                }
                comparison = true;
            }
            let right = self.expression(r)?;
            left = form(offset, operator, vec![left, right])?;
        }
        Ok(left)
    }
    /// Comma-separated constructor/call arguments and fixed array elements allow a trailing comma.
    fn arguments(&mut self, close: &'static str) -> Result<Vec<Node<'a>>, Diagnostic> {
        let mut args = Vec::new();
        while !self.take(Kind::Symbol(close)) {
            args.push(self.expression(0)?);
            if self.take(Kind::Symbol(close)) {
                break;
            }
            self.expect(
                Kind::Symbol(","),
                "expected ',' between arguments; keyword arguments and comprehensions are unsupported",
            )?;
        }
        Ok(args)
    }
}
