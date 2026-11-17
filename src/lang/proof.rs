//! The binding facts every admission relies on, each proved in one place.
//!
//! A fact is a token only this module constructs, so code holding one has run the whole check, not
//! a local copy of part of it. Callers keep their own work charges; these functions charge nothing.

use super::scopes::{Name, Reference, Scopes, SymbolId};
use super::tree::{NodeId, Span, Tree};
use std::collections::HashMap;

/// No resolved reference assigns the binding: plain, compound and logical assignment, updates,
/// and destructuring or `for…in`/`for…of` targets are all writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unwritten(());

/// The binding is [`Unwritten`] and has a single runtime declaration, so no `var` or function
/// redeclaration and no enum or namespace merge gives it a second value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Immutable(());

/// A read that can only run once its declarator has initialized the binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Initialized(());

/// The references that assign the binding, in the sense of [`Unwritten`].
pub fn writes(scopes: &Scopes, symbol: SymbolId) -> impl Iterator<Item = &Reference> {
    scopes
        .symbol(symbol)
        .references
        .iter()
        .filter(|reference| reference.write)
}

/// Proves [`Unwritten`].
pub fn unwritten(scopes: &Scopes, symbol: SymbolId) -> Option<Unwritten> {
    writes(scopes, symbol)
        .next()
        .is_none()
        .then_some(Unwritten(()))
}

/// How a symbol is declared among runtime identifiers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Declared {
    /// Binding identifiers naming it, counting the same span each time the tree repeats it.
    pub occurrences: usize,
    /// The first binding span.
    first: Option<Span>,
    /// Whether a second, different binding span declares it.
    redeclared: bool,
}

impl Declared {
    /// Exactly one binding span declares the symbol.
    pub fn single(&self) -> bool {
        self.first.is_some() && !self.redeclared
    }

    /// Records one binding identifier at `span`.
    fn add(&mut self, span: Span) {
        self.occurrences += 1;
        match self.first {
            None => self.first = Some(span),
            Some(first) => self.redeclared |= first != span,
        }
    }
}

/// Binding identifiers that declare a runtime value: an erased overload signature does not.
fn runtime_bindings<'a>(
    tree: &'a Tree,
    scopes: &'a Scopes,
) -> impl Iterator<Item = (SymbolId, Span)> + 'a {
    tree.nodes().filter_map(move |node| {
        if tree.kind(node) != "Identifier"
            || tree
                .parent(node)
                .is_some_and(|parent| tree.kind(parent) == "TSDeclareFunction")
        {
            return None;
        }
        let span = tree.span(node);
        match scopes.name_at(span.start) {
            Some(Name::Binding(Some(symbol))) => Some((symbol, span)),
            _ => None,
        }
    })
}

/// How `symbol` is declared, from one walk of the tree.
pub fn declared(tree: &Tree, scopes: &Scopes, symbol: SymbolId) -> Declared {
    let mut declared = Declared::default();
    for (_, span) in runtime_bindings(tree, scopes).filter(|&(binding, _)| binding == symbol) {
        declared.add(span);
    }
    declared
}

/// How every symbol is declared, from one walk of the tree, for callers asking about many.
#[derive(Debug, Default)]
pub struct Declarations(HashMap<SymbolId, Declared>);

impl Declarations {
    /// Indexes `parsed`.
    pub fn of(tree: &Tree, scopes: &Scopes) -> Self {
        let mut declarations = HashMap::<SymbolId, Declared>::new();
        for (symbol, span) in runtime_bindings(tree, scopes) {
            declarations.entry(symbol).or_default().add(span);
        }
        Self(declarations)
    }

    /// How `symbol` is declared; a symbol no runtime identifier binds has no declaration.
    pub fn get(&self, symbol: SymbolId) -> Declared {
        self.0.get(&symbol).copied().unwrap_or_default()
    }

    /// How many symbols the index holds.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

/// Proves [`Immutable`] from the symbol's [`Declared`] record.
pub fn immutable(scopes: &Scopes, symbol: SymbolId, declared: Declared) -> Option<Immutable> {
    unwritten(scopes, symbol)?;
    declared.single().then_some(Immutable(()))
}

/// Proves [`Initialized`] from position alone: `read` starts after `declarator` ends, lies inside
/// the declaration's parent (its statement list or `for` head, so not in a later `switch` case,
/// which control can enter past the declaration, nor a `for…of`/`for…in` iterable, which runs
/// before the binding exists), and no function declaration, which hoisting
/// can call before the declaration runs, lies between them. A read inside a function declaration
/// is never proved here; a caller that proves when that function runs owns that case.
pub fn initialized(tree: &Tree, declarator: NodeId, read: NodeId) -> Option<Initialized> {
    let enclosing = tree.parent(tree.parent(declarator)?)?;
    if tree.span(read).start < tree.span(declarator).end {
        return None;
    }
    // A `for…of`/`for…in` iterable runs before the loop binds its declaration.
    if matches!(tree.kind(enclosing), "ForOfStatement" | "ForInStatement")
        && tree
            .child(enclosing, "right")
            .is_some_and(|right| tree.span(right).contains(tree.span(read)))
    {
        return None;
    }
    for ancestor in tree.ancestors(read) {
        if ancestor == enclosing {
            return Some(Initialized(()));
        }
        if tree.kind(ancestor) == "FunctionDeclaration" {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests;
