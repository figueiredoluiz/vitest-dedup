//! Owned scope analysis: which declaration every identifier binds or reads.

use super::tree::{NodeId, Span};
use std::collections::HashMap;

/// A declared binding of a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub(super) u32);

/// What declares a symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Declaration {
    /// An import specifier.
    Import {
        /// The `ImportDeclaration`.
        import: NodeId,
        /// The specifier.
        specifier: NodeId,
        /// What it imports.
        imported: Imported,
    },
    /// A `VariableDeclarator`, of a `var`, `let`, `const` or `using` declaration.
    Variable(NodeId),
    /// A function declaration or a named function expression.
    Function(NodeId),
    /// A class declaration or a named class expression.
    Class(NodeId),
    /// Anything else (a parameter, a catch binding, a type, an enum): the binding identifier, when
    /// the tree has one.
    Other(Option<NodeId>),
}

/// The export an import specifier names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Imported {
    /// `import { name }` or `import { name as local }`.
    Named(String),
    /// `import local from`.
    Default,
    /// `import * as local from`.
    Namespace,
}

/// A use of a symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference {
    /// The identifier, when the tree has one at the reference's span.
    pub node: Option<NodeId>,
    /// The identifier's span.
    pub span: Span,
    /// Whether it assigns the symbol.
    pub write: bool,
}

/// A declared binding.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// The name as declared.
    pub name: String,
    /// The binding identifier's span.
    pub span: Span,
    /// What declares it.
    pub declaration: Declaration,
    /// Every resolved use, in source order.
    pub references: Vec<Reference>,
    /// Whether it is declared in the module's top-level scope.
    pub top_level: bool,
}

/// What an identifier at an offset is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Name {
    /// A declaration of the symbol, or of nothing the analysis records.
    Binding(Option<SymbolId>),
    /// A reference to the symbol, or to a global when `None`.
    Reference(Option<SymbolId>),
}

/// The symbols of a module, and what each identifier binds or reads.
#[derive(Debug, Default)]
pub struct Scopes {
    pub(super) symbols: Vec<Symbol>,
    pub(super) names: HashMap<u32, Name>,
    pub(super) top_level: HashMap<String, SymbolId>,
}

impl Scopes {
    /// The symbol `id` names.
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.0 as usize]
    }

    /// Every symbol.
    pub fn symbols(&self) -> impl Iterator<Item = SymbolId> {
        (0..self.symbols.len() as u32).map(SymbolId)
    }

    /// What the identifier starting at `offset` binds or reads, when it is one scope analysis
    /// recorded. A property name is neither.
    pub fn name_at(&self, offset: u32) -> Option<Name> {
        self.names.get(&offset).copied()
    }

    /// The symbol the identifier starting at `offset` reads or declares.
    pub fn symbol_at(&self, offset: u32) -> Option<SymbolId> {
        match self.name_at(offset)? {
            Name::Binding(symbol) | Name::Reference(symbol) => symbol,
        }
    }

    /// Whether the identifier starting at `offset` reads a name no scope declares.
    pub fn is_global_at(&self, offset: u32) -> bool {
        self.name_at(offset) == Some(Name::Reference(None))
    }

    /// The top-level symbol named `name`.
    pub fn top_level(&self, name: &str) -> Option<SymbolId> {
        self.top_level.get(name).copied()
    }

    /// The node a symbol's declaration starts from: the specifier, declarator, function, class or
    /// binding identifier.
    pub fn declaration_node(&self, id: SymbolId) -> Option<NodeId> {
        match &self.symbol(id).declaration {
            Declaration::Import { specifier, .. } => Some(*specifier),
            Declaration::Variable(node)
            | Declaration::Function(node)
            | Declaration::Class(node) => Some(*node),
            Declaration::Other(node) => *node,
        }
    }
}
