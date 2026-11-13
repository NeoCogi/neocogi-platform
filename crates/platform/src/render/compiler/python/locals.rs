//! Lexical assignment resolution and inferred mutability, before shared type validation.
use super::{Diagnostic, Node, Value, form};
use std::collections::{BTreeMap, BTreeSet};

/// Parameter writes get a private local copy; ordinary locals identify their original declaration.
#[derive(Clone, Copy)]
enum Binding {
    /// Immutable incoming value, copied into a mutable local only when assigned in this frontend.
    Parameter,
    /// Declaration byte position, unique among handwritten binding statements.
    Local(usize),
}
/// One function's lexical names and writes; shader resources remain the semantic checker's authority.
struct Resolver {
    /// Innermost scope is last. Names declared in a suite do not escape it.
    scopes: Vec<BTreeMap<String, Binding>>,
    /// Original declarations requiring mutable storage.
    written: BTreeSet<usize>,
    /// Incoming parameters requiring private value copies before the body executes.
    parameters: BTreeSet<String>,
}
/// Lower ordinary assignments to declarations or stores, then mark only written locals mutable.
pub(super) fn resolve<'a>(body: &mut Vec<Node<'a>>, parameters: &[Node<'a>]) -> Result<(), Diagnostic> {
    let mut resolver = Resolver {
        scopes: vec![
            parameters
                .iter()
                .map(|p| Ok((p.list()?[0].atom()?.to_owned(), Binding::Parameter)))
                .collect::<Result<_, Diagnostic>>()?,
        ],
        written: BTreeSet::new(),
        parameters: BTreeSet::new(),
    };
    resolver.block(body)?;
    mark(body, &resolver.written)?;
    let mut copies = Vec::new();
    for parameter in parameters {
        let name = &parameter.list()?[0];
        if resolver.parameters.contains(name.atom()?) {
            // Shared validation resolves the initializer to the outer parameter,
            // then publishes this same-named local in the body's inner scope.
            copies.push(form(parameter.offset, "var", vec![name.clone(), parameter.list()?[1].clone(), name.clone()])?);
        }
    }
    copies.append(body);
    *body = copies;
    Ok(())
}
/// A write path must retain its root; the shared checker owns final lvalue/type admission.
fn root<'a, 'b>(node: &'b Node<'a>) -> Option<&'b str> {
    if let Ok(name) = node.atom() {
        return Some(name);
    }
    let values = node.list().ok()?;
    if values.len() == 3 && matches!(values[0].atom().ok(), Some("." | "index")) {
        root(&values[1])
    } else {
        None
    }
}
impl Resolver {
    /// Declare once in this exact lexical scope; explicit annotations may shadow an outer name.
    fn declare(&mut self, name: &Node<'_>, offset: usize) -> Result<(), Diagnostic> {
        let name_text = name.atom()?;
        if self.scopes.last().unwrap().contains_key(name_text) {
            return Err(name.error("duplicate local declaration in this block"));
        }
        self.scopes.last_mut().unwrap().insert(name_text.to_owned(), Binding::Local(offset));
        Ok(())
    }
    /// Every nested branch/loop owns a new scope, matching the existing typed IR contract.
    fn block(&mut self, statements: &mut [Node<'_>]) -> Result<(), Diagnostic> {
        self.scopes.push(BTreeMap::new());
        for statement in statements {
            let offset = statement.offset;
            let Value::List(values) = &mut statement.value else {
                unreachable!("parser emits statement forms")
            };
            match values[0].atom()? {
                "$assign" => {
                    let plain = values[1].atom().is_ok();
                    let binding = root(&values[1]).and_then(|name| self.scopes.iter().rev().find_map(|scope| scope.get(name).copied()));
                    if binding.is_none() && plain {
                        self.declare(&values[1], offset)?;
                        values[0] = Node::word(offset, "let");
                    } else {
                        match binding {
                            Some(Binding::Local(offset)) => {
                                self.written.insert(offset);
                            }
                            Some(Binding::Parameter) => {
                                self.parameters.insert(root(&values[1]).unwrap().to_owned());
                            }
                            None => {}
                        }
                        values[0] = Node::word(offset, "set");
                    }
                }
                "$declare" => {
                    self.declare(&values[1], offset)?;
                    values[0] = Node::word(offset, if values.len() == 3 { "var" } else { "let" });
                }
                "let" => self.declare(&values[1], offset)?, // Compiler-generated eager snapshots.
                "if" => {
                    for branch in &mut values[2..] {
                        let Value::List(block) = &mut branch.value else { unreachable!() };
                        self.block(&mut block[1..])?;
                    }
                }
                "while" => self.block(&mut values[2..])?,
                _ => {}
            }
        }
        self.scopes.pop();
        Ok(())
    }
}
/// Update declaration storage only after all writes, including later nested writes, are known.
fn mark(statements: &mut [Node<'_>], written: &BTreeSet<usize>) -> Result<(), Diagnostic> {
    for statement in statements {
        let Value::List(values) = &mut statement.value else { unreachable!() };
        match values[0].atom()? {
            "let" if written.contains(&statement.offset) => values[0] = Node::word(statement.offset, if values.len() == 3 { "$var" } else { "var" }),
            "if" => {
                for branch in &mut values[2..] {
                    let Value::List(block) = &mut branch.value else { unreachable!() };
                    mark(&mut block[1..], written)?;
                }
            }
            "while" => mark(&mut values[2..], written)?,
            _ => {}
        }
    }
    Ok(())
}
