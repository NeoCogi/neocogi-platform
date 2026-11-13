//! Indentation-aware ASCII tokens retaining original UTF-8 byte positions.
use super::super::Diagnostic;

/// Structural tokens are explicit; indentation never uses sentinel spellings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind<'a> {
    /// Identifier or language keyword, borrowed from source.
    Word(&'a str),
    /// Decimal scalar spelling; sign remains a separate operator.
    Number(&'a str),
    /// Delimiter or longest matching operator.
    Symbol(&'a str),
    /// End of a logical line, outside brackets.
    Newline,
    /// Start of a more deeply indented suite.
    Indent,
    /// Return to a previously established indentation level.
    Dedent,
    /// Complete input, including all outstanding dedents.
    End,
}
/// One token always points into the original source, including synthetic suite boundaries.
#[derive(Clone, Copy, Debug)]
pub(super) struct Token<'a> {
    /// First byte, suitable for a source diagnostic.
    pub offset: usize,
    /// Borrowed spelling or explicit structural state.
    pub kind: Kind<'a>,
}
/// Lex complete input before parsing; token and bracket budgets bound scratch work.
pub(super) fn tokens(source: &str) -> Result<Vec<Token<'_>>, Diagnostic> {
    let mut lexer = Lexer {
        source,
        cursor: 0,
        line_start: true,
        indents: vec![0],
        brackets: Vec::new(),
        output: Vec::new(),
    };
    lexer.run()?;
    Ok(lexer.output)
}
/// Owns lexical state; bracket continuation suppresses indentation and logical newlines.
struct Lexer<'a> {
    /// Original text, never normalized before diagnostics.
    source: &'a str,
    /// Next byte; non-ASCII text is accepted only inside comments.
    cursor: usize,
    /// True before indentation on a physical line has been consumed.
    line_start: bool,
    /// Strictly increasing columns, beginning with module level zero.
    indents: Vec<usize>,
    /// Opening delimiter and location, used to reject mismatches and unterminated input.
    brackets: Vec<(u8, usize)>,
    /// Owned bounded token stream.
    output: Vec<Token<'a>>,
}
impl<'a> Lexer<'a> {
    /// Reject tabs rather than assigning platform-dependent indentation widths.
    fn error(&self, message: &str) -> Diagnostic {
        Diagnostic {
            offset: self.cursor,
            message: message.into(),
        }
    }
    /// Every token is accounted, including generated indentation boundaries.
    fn push(&mut self, offset: usize, kind: Kind<'a>) -> Result<(), Diagnostic> {
        if self.output.len() >= 200_000 {
            return Err(self.error("syntax token limit exceeded"));
        }
        self.output.push(Token { offset, kind });
        Ok(())
    }
    /// Blank/comment-only lines have no effect on a suite's indentation.
    fn indentation(&mut self) -> Result<(), Diagnostic> {
        let start = self.cursor;
        while self.source.as_bytes().get(self.cursor) == Some(&b' ') {
            self.cursor += 1;
        }
        if self.source.as_bytes().get(self.cursor) == Some(&b'\t') {
            return Err(self.error("tabs are unsupported; use spaces for indentation"));
        }
        if !self.brackets.is_empty() || matches!(self.source.as_bytes().get(self.cursor), None | Some(b'#' | b'\r' | b'\n')) {
            return Ok(());
        }
        let width = self.cursor - start;
        if width > *self.indents.last().unwrap() {
            if self.indents.len() >= 96 {
                return Err(self.error("syntax complexity limit exceeded"));
            }
            self.indents.push(width);
            self.push(self.cursor, Kind::Indent)?;
        } else {
            while width < *self.indents.last().unwrap() {
                self.indents.pop();
                self.push(self.cursor, Kind::Dedent)?;
            }
            if width != *self.indents.last().unwrap() {
                return Err(self.error("dedent does not match an enclosing indentation level"));
            }
        }
        Ok(())
    }
    /// Scan decimal/exponent syntax without consuming a following member-access dot.
    fn number(&mut self) -> Result<(), Diagnostic> {
        let offset = self.cursor;
        let bytes = self.source.as_bytes();
        while bytes.get(self.cursor).is_some_and(u8::is_ascii_digit) {
            self.cursor += 1;
        }
        if bytes.get(self.cursor) == Some(&b'.') {
            self.cursor += 1;
            while bytes.get(self.cursor).is_some_and(u8::is_ascii_digit) {
                self.cursor += 1;
            }
        }
        if matches!(bytes.get(self.cursor), Some(b'e' | b'E')) {
            self.cursor += 1;
            if matches!(bytes.get(self.cursor), Some(b'+' | b'-')) {
                self.cursor += 1;
            }
            let start = self.cursor;
            while bytes.get(self.cursor).is_some_and(u8::is_ascii_digit) {
                self.cursor += 1;
            }
            if start == self.cursor {
                return Err(self.error("exponent requires decimal digits"));
            }
        }
        if bytes.get(self.cursor).is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_') {
            return Err(self.error("invalid numeric suffix; use u32(...) for unsigned literals"));
        }
        self.push(offset, Kind::Number(&self.source[offset..self.cursor]))
    }
    /// Longest matching punctuation prevents +=, ** and shifts from splitting ambiguously.
    fn symbol(&mut self) -> Result<(), Diagnostic> {
        let offset = self.cursor;
        let tail = &self.source[offset..];
        let length = [
            "**=", "<<=", ">>=", "->", "==", "!=", "<=", ">=", "<<", ">>", "**", "+=", "-=", "*=", "/=", "%=", "@=", "&=", "|=", "^=",
        ]
        .iter()
        .find(|symbol| tail.starts_with(**symbol))
        .map_or(1, |symbol| symbol.len());
        let byte = self.source.as_bytes()[offset];
        match byte {
            b'(' | b'[' => {
                if self.brackets.len() >= 96 {
                    return Err(self.error("syntax complexity limit exceeded"));
                }
                self.brackets.push((byte, offset));
            }
            b')' | b']' => {
                let expected = if byte == b')' { b'(' } else { b'[' };
                if self.brackets.pop().is_none_or(|(open, _)| open != expected) {
                    return Err(self.error("mismatched closing delimiter"));
                }
            }
            b':' | b',' | b'.' | b'+' | b'-' | b'*' | b'/' | b'%' | b'@' | b'&' | b'|' | b'^' | b'~' | b'<' | b'>' | b'=' | b'!' => {}
            b'\'' | b'"' => return Err(self.error("strings are unsupported in shader source")),
            b'\t' => return Err(self.error("tabs are unsupported; use spaces")),
            _ => return Err(self.error("unexpected character in Python-like shader source")),
        }
        self.cursor += length;
        self.push(offset, Kind::Symbol(&self.source[offset..self.cursor]))
    }
    /// At EOF every outstanding indentation is closed, while open brackets reject at their origin.
    fn run(&mut self) -> Result<(), Diagnostic> {
        let bytes = self.source.as_bytes();
        while self.cursor < bytes.len() {
            if self.line_start {
                self.indentation()?;
                self.line_start = false;
                if self.cursor == bytes.len() {
                    break;
                }
            }
            match bytes[self.cursor] {
                b' ' => self.cursor += 1,
                b'#' => {
                    while bytes.get(self.cursor).is_some_and(|b| *b != b'\n') {
                        self.cursor += 1;
                    }
                }
                b'\r' | b'\n' => {
                    let offset = self.cursor;
                    if bytes[self.cursor] == b'\r' {
                        self.cursor += 1;
                        if bytes.get(self.cursor) != Some(&b'\n') {
                            return Err(self.error("expected LF after CR"));
                        }
                    }
                    self.cursor += 1;
                    if self.brackets.is_empty() && !matches!(self.output.last().map(|t| t.kind), None | Some(Kind::Newline | Kind::Indent | Kind::Dedent)) {
                        self.push(offset, Kind::Newline)?;
                    }
                    self.line_start = true;
                }
                b'0'..=b'9' => self.number()?,
                b'.' if bytes.get(self.cursor + 1).is_some_and(u8::is_ascii_digit) => self.number()?,
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    let offset = self.cursor;
                    while bytes.get(self.cursor).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
                        self.cursor += 1;
                    }
                    self.push(offset, Kind::Word(&self.source[offset..self.cursor]))?;
                }
                _ => self.symbol()?,
            }
        }
        if let Some(&(_, offset)) = self.brackets.first() {
            return Err(Diagnostic {
                offset,
                message: "unclosed delimiter".into(),
            });
        }
        self.push(self.cursor, Kind::Newline)?;
        while self.indents.len() > 1 {
            self.indents.pop();
            self.push(self.cursor, Kind::Dedent)?;
        }
        self.push(self.cursor, Kind::End)
    }
}
