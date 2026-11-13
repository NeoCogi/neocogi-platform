//! Shared bounded syntax tree and source-preserving frontend selection.

use super::Diagnostic;
use std::borrow::Cow;

/// An atom or parenthesized sequence with its original source position.
#[derive(Clone, Debug)]
pub(crate) struct Node<'a> {
    /// First byte of this form, used for diagnostics after parsing.
    pub offset: usize,
    /// Borrowed spelling or owned child sequence.
    pub value: Value<'a>,
    /// Intrinsic tree height; every frontend bounds generated as well as written nesting.
    height: u8,
}

/// Syntax has no strings, escapes, quoting, or implicit evaluation.
#[derive(Clone, Debug)]
pub(crate) enum Value<'a> {
    /// A whitespace-delimited name, operator, or literal.
    Atom(Cow<'a, str>),
    /// A parenthesized sequence.
    List(Vec<Node<'a>>),
}

impl<'a> Node<'a> {
    /// Reject a list where an atom is required.
    pub fn atom(&self) -> Result<&str, Diagnostic> {
        match &self.value {
            Value::Atom(value) => Ok(value),
            _ => Err(self.error("expected an atom")),
        }
    }

    /// Ordinary spellings borrow the source; a frontend may own a normalized literal or fresh name.
    pub fn word(offset: usize, value: impl Into<Cow<'a, str>>) -> Self {
        Self {
            offset,
            value: Value::Atom(value.into()),
            height: 0,
        }
    }

    /// Enforce the same finite tree height for both source and frontend-generated forms.
    pub fn form(offset: usize, children: Vec<Self>) -> Result<Self, Diagnostic> {
        let height = children.iter().map(|node| node.height).max().unwrap_or(0) + 1;
        if height > 96 {
            return Err(Diagnostic {
                offset,
                message: "syntax complexity limit exceeded".into(),
            });
        }
        Ok(Self {
            offset,
            value: Value::List(children),
            height,
        })
    }

    /// Reject an atom where a list is required.
    pub fn list(&self) -> Result<&[Node<'a>], Diagnostic> {
        match &self.value {
            Value::List(values) => Ok(values),
            _ => Err(self.error("expected a parenthesized form")),
        }
    }

    /// Attach a semantic diagnostic to this form's start.
    pub fn error(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            offset: self.offset,
            message: message.into(),
        }
    }

    /// Read a nonnegative decimal profile index.
    pub fn number(&self) -> Result<u32, Diagnostic> {
        self.atom()?.parse().map_err(|_| self.error("expected an unsigned decimal integer"))
    }
}

/// A leading parenthesis selects S-expressions, after their whitespace/comments.
/// Other input selects Python-like syntax. No generated source string loses locations.
pub(crate) fn parse(source: &str) -> Result<Node<'_>, Diagnostic> {
    let mut prefix = Reader { source, cursor: 0, nodes: 0 };
    prefix.space();
    if prefix.cursor < source.len() && source.as_bytes().get(prefix.cursor) != Some(&b'(') {
        #[cfg(feature = "shader-python")]
        return super::python::parse(source);
        #[cfg(not(feature = "shader-python"))]
        return Err(Diagnostic {
            offset: prefix.cursor,
            message: "Python-like shader frontend is disabled; enable the 'shader-python' Cargo feature".into(),
        });
    }
    sexp(source)
}

/// Parses exactly one `(module ...)` form with bounded nesting and node count.
fn sexp(source: &str) -> Result<Node<'_>, Diagnostic> {
    let mut reader = Reader { source, cursor: 0, nodes: 0 };
    let root = reader.node(0)?;
    reader.space();
    if reader.cursor != source.len() {
        return Err(Diagnostic {
            offset: reader.cursor,
            message: "unexpected form after module".into(),
        });
    }
    let values = root.list()?;
    if values.first().and_then(|node| node.atom().ok()) != Some("module") {
        return Err(root.error("expected (module ...)"));
    }
    Ok(root)
}

/// Owns parser accounting; every recursion consumes a nesting allowance.
struct Reader<'a> {
    /// Entire immutable UTF-8 source.
    source: &'a str,
    /// Next unread byte, always at a character boundary.
    cursor: usize,
    /// Total forms, capped to bound downstream allocation and traversal.
    nodes: usize,
}

impl<'a> Reader<'a> {
    /// Skips ASCII whitespace and semicolon comments through the next newline.
    fn space(&mut self) {
        let bytes = self.source.as_bytes();
        while let Some(&byte) = bytes.get(self.cursor) {
            if byte.is_ascii_whitespace() {
                self.cursor += 1;
            } else if byte == b';' {
                while bytes.get(self.cursor).is_some_and(|&b| b != b'\n') {
                    self.cursor += 1;
                }
            } else {
                break;
            }
        }
    }

    /// Reads a form without accepting unmatched delimiters or non-ASCII atoms.
    fn node(&mut self, depth: usize) -> Result<Node<'a>, Diagnostic> {
        self.space();
        let offset = self.cursor;
        let error = |message: &str| Diagnostic { offset, message: message.into() };
        self.nodes += 1;
        if depth > 96 || self.nodes > 100_000 {
            return Err(error("syntax complexity limit exceeded"));
        }
        let bytes = self.source.as_bytes();
        match bytes.get(self.cursor) {
            Some(b'(') => {
                self.cursor += 1;
                let mut children = Vec::new();
                loop {
                    self.space();
                    match bytes.get(self.cursor) {
                        Some(b')') => {
                            self.cursor += 1;
                            break;
                        }
                        None => return Err(error("unclosed parenthesis")),
                        _ => children.push(self.node(depth + 1)?),
                    }
                }
                Node::form(offset, children)
            }
            Some(b')') => Err(error("unexpected closing parenthesis")),
            None => Err(error("expected a form")),
            _ => {
                while let Some(&byte) = bytes.get(self.cursor) {
                    if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b';') {
                        break;
                    }
                    if !byte.is_ascii_graphic() || matches!(byte, b'"' | b'\'' | b'`' | b'\\') {
                        return Err(error("atoms must use plain ASCII; strings and quoting are unsupported"));
                    }
                    self.cursor += 1;
                }
                Ok(Node::word(offset, &self.source[offset..self.cursor]))
            }
        }
    }
}
