//! Bounded primitive proofs shared by row binding, equality and title inputs.

use super::{Normalizer, Setting, Structure};
use crate::lang::{Declaration, Field, Imported, NodeId, Parsed, Scalar, SymbolId};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Per-proof source and recursion bounds; failure never supplies a partial value.
const MAX_SOURCE_BYTES: usize = 512 * 1024;

/// A borrowed module view; all identifiers still belong to that module's scopes.
#[derive(Clone, Copy)]
struct View<'a> {
    tree: &'a crate::lang::Tree,
    scopes: &'a crate::lang::Scopes,
}

/// Every reference is a static member read, without mutation, receiver execution or escape.
fn readonly_members(tree: &crate::lang::Tree, symbol: &crate::lang::Symbol) -> bool {
    !symbol.references.iter().any(|r| {
        let Some(id) = r.node else {
            return true;
        };
        let Some(member) = tree.parent(id) else {
            return true;
        };
        if r.write
            || tree.kind(member) != "MemberExpression"
            || tree.child(member, "object") != Some(id)
        {
            return true;
        }
        if tree.flag(member, "computed")
            && tree
                .child(member, "property")
                .is_none_or(|p| tree.kind(p) != "Literal")
        {
            return true;
        }
        let receiver = |mut head| loop {
            head = tree.inner(head);
            if head == member {
                return true;
            }
            if tree.kind(head) != "MemberExpression" {
                return false;
            }
            let Some(object) = tree.child(head, "object") else {
                return false;
            };
            head = object;
        };
        tree.ancestors(member).any(|a| match tree.kind(a) {
            "AssignmentExpression" | "ForOfStatement" | "ForInStatement" => tree
                .child(a, "left")
                .is_some_and(|l| tree.span(l).contains(tree.span(member))),
            "UpdateExpression" => tree.child(a, "argument") == Some(member),
            "UnaryExpression" => {
                tree.text(a, "operator") == Some("delete")
                    && tree.child(a, "argument") == Some(member)
            }
            "CallExpression" | "NewExpression" => tree.child(a, "callee").is_some_and(receiver),
            "TaggedTemplateExpression" => tree.child(a, "tag").is_some_and(receiver),
            _ => false,
        })
    })
}

/// One primitive read owns its work budget and import recursion set.
struct Reader<'a> {
    setting: Setting<'a>,
    budget: usize,
    bytes: usize,
    active: HashSet<(PathBuf, String)>,
    imports_safe: bool,
}

impl Normalizer<'_> {
    /// Gives one fresh normalizer the enclosing suite invocation's proven primitive inputs.
    pub(crate) fn for_suite_values(mut self, values: HashMap<SymbolId, Scalar>) -> Self {
        self.row_values = values;
        self
    }

    /// Binds primitives and isolates readonly literal arrays without claiming their equality.
    pub(crate) fn suite_row_values(
        &mut self,
        function: NodeId,
        row: &super::Row,
    ) -> Option<(HashMap<SymbolId, Scalar>, bool)> {
        let tree = self.tree;
        if tree
            .child(function, "body")
            .is_none_or(|body| crate::lang::uses_receiver(tree, body))
        {
            return None;
        }
        let parameters = tree
            .list(function, "params")
            .into_iter()
            .collect::<Option<Vec<_>>>()?;
        let properties = match row {
            super::Row::Fields(p) | super::Row::Whole(_, Some(p)) => Some(p),
            _ => None,
        };
        let cells = match row {
            super::Row::Spread(cells) => cells.clone(),
            super::Row::Whole(_, Some(p)) | super::Row::Fields(p) => {
                p.iter().map(|(_, n)| *n).collect()
            }
            super::Row::Whole(node, None) => vec![*node],
        };
        let values = cells
            .iter()
            .map(|&n| self.row_scalar(n))
            .collect::<Vec<_>>();
        if cells
            .iter()
            .zip(&values)
            .any(|(&node, value)| value.is_none() && !self.primitive_array(node))
        {
            return None;
        }
        let mut bound = HashMap::new();
        let mut isolated = false;
        for (i, parameter) in parameters.into_iter().enumerate() {
            if tree.kind(parameter) == "Identifier" {
                if properties.is_some() {
                    return None;
                }
                let symbol = self.scopes.symbol_at(tree.span(parameter).start)?;
                if self
                    .scopes
                    .symbol(symbol)
                    .references
                    .iter()
                    .any(|r| r.write)
                {
                    return None;
                }
                match values.get(i) {
                    Some(Some(value)) => {
                        bound.insert(symbol, value.clone());
                    }
                    Some(None) => {
                        if !self.readonly_array(symbol, cells[i]) {
                            return None;
                        }
                        isolated |= !self.scopes.symbol(symbol).references.is_empty();
                    }
                    None => {
                        bound.insert(symbol, Scalar::Undefined);
                    }
                }
            } else if i == 0 && tree.kind(parameter) == "ObjectPattern" {
                let fields = properties?;
                for property in tree.list(parameter, "properties").into_iter().flatten() {
                    if tree.kind(property) != "Property" || tree.flag(property, "computed") {
                        return None;
                    }
                    let key = crate::lang::property_name(tree, property)?;
                    if matches!(key, "__proto__" | "constructor" | "toString" | "valueOf") {
                        return None;
                    }
                    let binding = tree.child(property, "value")?;
                    if tree.kind(binding) != "Identifier" {
                        return None;
                    }
                    let symbol = self.scopes.symbol_at(tree.span(binding).start)?;
                    if self
                        .scopes
                        .symbol(symbol)
                        .references
                        .iter()
                        .any(|r| r.write)
                    {
                        return None;
                    }
                    let value = match fields.iter().rev().find(|(name, _)| name == key) {
                        Some((_, node)) => match self.row_scalar(*node) {
                            Some(value) => value,
                            None => {
                                if !self.readonly_array(symbol, *node) {
                                    return None;
                                }
                                isolated |= !self.scopes.symbol(symbol).references.is_empty();
                                continue;
                            }
                        },
                        None if super::rows::OBJECT_PROTOTYPE.contains(&key) => return None,
                        None => Scalar::Undefined,
                    };
                    bound.insert(symbol, value);
                }
            } else {
                return None;
            }
        }
        Some((bound, isolated))
    }

    /// A bounded array construction has no unknown reads, elisions, spreads or object aliases.
    fn primitive_array(&mut self, node: NodeId) -> bool {
        let tree = self.tree;
        let node = tree.inner(node);
        tree.kind(node) == "ArrayExpression"
            && tree.list(node, "elements").len() <= 256
            && tree
                .list(node, "elements")
                .into_iter()
                .all(|n| n.is_some_and(|n| self.row_scalar(n).is_some()))
    }

    /// A composite binding stays private and every use reads an own scalar index or length.
    fn readonly_array(&mut self, symbol: SymbolId, node: NodeId) -> bool {
        if !self.primitive_array(node) {
            return false;
        }
        let tree = self.tree;
        let node = tree.inner(node);
        let symbol = self.scopes.symbol(symbol);
        if !readonly_members(tree, symbol) {
            return false;
        }
        let length = tree.list(node, "elements").len();
        symbol.references.iter().all(|reference| {
            let member = tree
                .parent(reference.node.expect("readonly reference"))
                .expect("readonly member");
            let property = tree
                .child(member, "property")
                .expect("a member has a property");
            if crate::lang::member_name(tree, member) == Some("length") {
                return true;
            }
            if !tree.flag(member, "computed") {
                return false;
            }
            match tree.field_of(property, "value") {
                Field::Value(serde_json::Value::Number(n)) => n
                    .as_f64()
                    .is_some_and(|n| n >= 0.0 && n.fract() == 0.0 && n < length as f64),
                Field::Value(serde_json::Value::String(key)) => key
                    .parse::<usize>()
                    .is_ok_and(|i| i < length && i.to_string() == *key),
                _ => false,
            }
        })
    }

    /// Supplies typed primitive arguments while preserving the existing partial JSON field reads.
    pub(crate) fn render_table_title(
        &mut self,
        parsed: &Parsed,
        pattern: &str,
        row: &super::TableRow,
        whole: bool,
        mode: crate::framework::TitleMode,
    ) -> Option<String> {
        use crate::framework::TitleValue;
        let (arguments, fields) = super::titles::row(parsed, &row.value, whole);
        let mut arguments = arguments
            .into_iter()
            .map(|v| v.map(TitleValue::Json))
            .collect::<Vec<_>>();
        let mut fields = fields.map(|f| {
            f.into_iter()
                .map(|(k, v)| (k, TitleValue::Json(v)))
                .collect::<HashMap<_, _>>()
        });
        let tree = self.tree;
        let scalar_whole = whole
            && matches!(&row.value,super::Row::Whole(node,_) if tree.kind(tree.inner(*node))!="ArrayExpression");
        let cells = match &row.value {
            super::Row::Spread(cells) => cells.iter().copied().map(Some).collect(),
            super::Row::Whole(node, _) if tree.kind(tree.inner(*node)) == "ArrayExpression" => tree
                .list(tree.inner(*node), "elements")
                .into_iter()
                .collect(),
            super::Row::Whole(node, _) => vec![Some(*node)],
            super::Row::Fields(_) => Vec::new(),
        };
        for (i, node) in cells.into_iter().enumerate() {
            let value = match node {
                Some(node) => self.row_scalar(node),
                None => Some(Scalar::Undefined),
            };
            if let Some(value) = value {
                if scalar_whole && matches!(value, Scalar::Null | Scalar::Undefined) {
                    arguments.clear();
                    continue;
                }
                if let Some(argument) = arguments.get_mut(i) {
                    *argument = Some(TitleValue::Scalar(value));
                }
            }
        }
        let properties = match &row.value {
            super::Row::Fields(p) | super::Row::Whole(_, Some(p)) => Some(p),
            _ => None,
        };
        if let Some(properties) = properties {
            let fields = fields.get_or_insert_with(HashMap::new);
            let mut resolved = HashMap::new();
            for (key, node) in properties {
                resolved.insert(key, self.row_scalar(*node));
            }
            for (key, value) in resolved {
                if let Some(value) = value {
                    fields.insert(key.clone(), TitleValue::Scalar(value));
                }
            }
        }
        (self.setting.dialect.render_values)(pattern, &arguments, fields.as_ref(), row.index, mode)
    }

    /// Proves a primitive without evaluating arbitrary expressions or mutable objects.
    pub(crate) fn row_scalar(&mut self, node: NodeId) -> Option<Scalar> {
        if self.structure.direct_eval {
            return None;
        }
        let imports_safe = if let Some(safe) = self.scalar_imports_safe {
            safe
        } else {
            let mut visited = HashSet::new();
            let mut bytes = 0;
            let safe = !self.structure.setup_access
                && safe_graph(self.path, self.tree, self.setting, &mut visited, &mut bytes);
            self.scalar_imports_safe = Some(safe);
            safe
        };
        let parsed = View {
            tree: self.tree,
            scopes: self.scopes,
        };
        Reader {
            setting: self.setting,
            budget: 256,
            bytes: 0,
            active: HashSet::new(),
            imports_safe,
        }
        .read(parsed, self.path, node, &self.row_values)
    }

    /// Writes the proven value with the ordinary writer, preserving literal/type normal forms.
    pub(crate) fn scalar_form(&self, scalar: &Scalar, structural: bool) -> Option<String> {
        let source = format!("({});", scalar.source());
        let parsed = crate::lang::parse(self.path, &source).ok()?;
        let statement = parsed
            .tree
            .list(parsed.tree.root(), "body")
            .first()
            .copied()??;
        let expression = parsed.tree.child(statement, "expression")?;
        let structure = Structure::default();
        let mut normalizer = Normalizer::new(self.path, &parsed, self.setting, &structure);
        Some(if structural {
            normalizer.shape(expression, HashMap::new(), false)
        } else {
            normalizer.of(expression)
        })
    }
}

/// Every eagerly loaded project dependency must be free of framework setup capability.
fn safe_graph(
    path: &Path,
    tree: &crate::lang::Tree,
    setting: Setting,
    visited: &mut HashSet<PathBuf>,
    bytes: &mut usize,
) -> bool {
    // Vitest exposes mutable module objects. Without a namespace escape proof, even a named
    // const import can be replaced through a sibling namespace or dynamic import.
    if tree.nodes().any(|node| match tree.kind(node) {
        "ImportExpression" => true,
        "ImportNamespaceSpecifier" => tree
            .parent(node)
            .is_none_or(|import| tree.text(import, "importKind") != Some("type")),
        "ExportNamespaceSpecifier" => true,
        "ExportAllDeclaration" => {
            tree.child(node, "exported").is_some() && tree.text(node, "exportKind") != Some("type")
        }
        _ => false,
    }) {
        return false;
    }
    if !visited.insert(path.to_owned()) {
        return true;
    }
    if visited.len() > 32 {
        return false;
    }
    for statement in tree.list(tree.root(), "body").into_iter().flatten() {
        if !matches!(
            tree.kind(statement),
            "ImportDeclaration" | "ExportNamedDeclaration" | "ExportAllDeclaration"
        ) || tree.text(statement, "importKind") == Some("type")
            || tree.text(statement, "exportKind") == Some("type")
        {
            continue;
        }
        let Some(specifier) = tree
            .child(statement, "source")
            .and_then(|n| tree.text(n, "value"))
        else {
            continue;
        };
        if setting.dialect.is_api_module(specifier) {
            continue;
        }
        let Some(file) = setting
            .exports
            .sources
            .resolve(path, specifier, setting.root)
        else {
            return false;
        };
        if visited.contains(&file) {
            continue;
        }
        let Some(source) = setting.exports.sources.read(&file) else {
            return false;
        };
        *bytes = bytes.saturating_add(source.len());
        if *bytes > MAX_SOURCE_BYTES {
            return false;
        }
        let Ok(parsed) = crate::lang::parse(&file, &source) else {
            return false;
        };
        if crate::inventory::direct_eval(&parsed)
            || crate::inventory::setup_access(
                &parsed,
                crate::inventory::Setting {
                    dialect: setting.dialect,
                    globals: false,
                    root: setting.root,
                },
            )
            || !safe_graph(&file, &parsed.tree, setting, visited, bytes)
        {
            return false;
        }
    }
    true
}

impl Reader<'_> {
    /// Reads only primitive constructions, immutable constants and disciplined own-member reads.
    fn read(
        &mut self,
        parsed: View<'_>,
        path: &Path,
        node: NodeId,
        bound: &HashMap<SymbolId, Scalar>,
    ) -> Option<Scalar> {
        if self.budget == 0 {
            return None;
        }
        self.budget -= 1;
        let tree = parsed.tree;
        let node = tree.inner(node);
        let result = match tree.kind(node) {
            "Literal" => {
                if let Some(bigint) = tree.text(node, "bigint") {
                    if bigint.is_empty() || !bigint.bytes().all(|b| b.is_ascii_digit()) {
                        return None;
                    }
                    let digits = bigint.trim_start_matches('0');
                    Scalar::BigInt(if digits.is_empty() { "0" } else { digits }.into())
                } else {
                    match tree.field_of(node, "value") {
                        Field::Value(serde_json::Value::String(v)) => Scalar::String(v.clone()),
                        Field::Value(serde_json::Value::Bool(v)) => Scalar::Bool(*v),
                        Field::Value(serde_json::Value::Number(v)) => Scalar::Number(v.as_f64()?),
                        Field::Value(serde_json::Value::Null)
                            if tree.text(node, "raw") == Some("null") =>
                        {
                            Scalar::Null
                        }
                        _ => return None,
                    }
                }
            }
            "Identifier" => {
                let Some(symbol) = parsed.scopes.symbol_at(tree.span(node).start) else {
                    return match tree.text(node, "name")? {
                        "undefined" => Some(Scalar::Undefined),
                        "NaN" => Some(Scalar::Number(f64::NAN)),
                        "Infinity" => Some(Scalar::Number(f64::INFINITY)),
                        _ => None,
                    };
                };
                if let Some(value) = bound.get(&symbol) {
                    return Some(value.clone());
                }
                self.binding(parsed, path, symbol, Some(node), bound)?
            }
            "UnaryExpression" => {
                let value = self.read(parsed, path, tree.child(node, "argument")?, bound)?;
                match (tree.text(node, "operator")?, value) {
                    ("void", _) => Scalar::Undefined,
                    ("+", Scalar::Number(v)) => Scalar::Number(v),
                    ("-", Scalar::Number(v)) => Scalar::Number(-v),
                    ("-", Scalar::BigInt(v)) => Scalar::BigInt(if v == "0" {
                        v
                    } else if let Some(n) = v.strip_prefix('-') {
                        n.to_owned()
                    } else {
                        format!("-{v}")
                    }),
                    _ => return None,
                }
            }
            "BinaryExpression" if tree.text(node, "operator") == Some("/") => {
                let Scalar::Number(left) =
                    self.read(parsed, path, tree.child(node, "left")?, bound)?
                else {
                    return None;
                };
                let Scalar::Number(right) =
                    self.read(parsed, path, tree.child(node, "right")?, bound)?
                else {
                    return None;
                };
                Scalar::Number(left / right)
            }
            "TemplateLiteral" if tree.list(node, "expressions").is_empty() => {
                let quasi = tree.list(node, "quasis").first().copied()??;
                let Field::Object(fields) = tree.field_of(quasi, "value") else {
                    return None;
                };
                let text = fields
                    .iter()
                    .find_map(|(key, value)| match (&**key, value) {
                        ("cooked", Field::Value(serde_json::Value::String(text))) => {
                            Some(text.clone())
                        }
                        _ => None,
                    })?;
                Scalar::String(text)
            }
            "MemberExpression" => self.member(parsed, path, node, bound)?,
            _ => return None,
        };
        match &result {
            Scalar::String(text) if text.contains('\u{fffd}') || text.len() > 65_536 => None,
            _ => Some(result),
        }
    }

    /// A const read must be after initialization and cannot cross a switch-case declaration.
    fn binding(
        &mut self,
        parsed: View<'_>,
        path: &Path,
        symbol: SymbolId,
        at: Option<NodeId>,
        bound: &HashMap<SymbolId, Scalar>,
    ) -> Option<Scalar> {
        let tree = parsed.tree;
        let symbol = parsed.scopes.symbol(symbol);
        if symbol.references.iter().any(|r| r.write) {
            return None;
        }
        match &symbol.declaration {
            Declaration::Variable(declaration) => {
                let parent = tree.parent(*declaration)?;
                if tree.text(parent, "kind") != Some("const")
                    || tree.flag(parent, "declare")
                    || tree.kind(tree.child(*declaration, "id")?) != "Identifier"
                {
                    return None;
                }
                if let Some(at) = at {
                    if tree.span(*declaration).end > tree.span(at).start {
                        return None;
                    }
                    let case = |node| tree.ancestors(node).find(|&p| tree.kind(p) == "SwitchCase");
                    if case(*declaration).is_some() && case(*declaration) != case(at) {
                        return None;
                    }
                }
                self.read(parsed, path, tree.child(*declaration, "init")?, bound)
            }
            Declaration::Import {
                import,
                specifier,
                imported,
            } if self.imports_safe => {
                if tree.text(*import, "importKind") == Some("type")
                    || tree.text(*specifier, "importKind") == Some("type")
                {
                    return None;
                }
                let name = match imported {
                    Imported::Named(n) => n.as_str(),
                    Imported::Default => "default",
                    Imported::Namespace => return None,
                };
                let source = tree.text(tree.child(*import, "source")?, "value")?;
                let file = self
                    .setting
                    .exports
                    .sources
                    .resolve(path, source, self.setting.root)?;
                self.exported(&file, name)
            }
            _ => None,
        }
    }

    /// Static member reads require a literal object with no observable mutation or escape route.
    fn member(
        &mut self,
        parsed: View<'_>,
        path: &Path,
        node: NodeId,
        bound: &HashMap<SymbolId, Scalar>,
    ) -> Option<Scalar> {
        let tree = parsed.tree;
        let object = tree.inner(tree.child(node, "object")?);
        if tree.kind(object) != "Identifier" {
            return None;
        }
        let symbol = parsed
            .scopes
            .symbol(parsed.scopes.symbol_at(tree.span(object).start)?);
        let property = tree.child(node, "property")?;
        let key = if !tree.flag(node, "computed") {
            tree.text(property, "name")?.to_owned()
        } else {
            match self.read(parsed, path, property, bound)? {
                Scalar::String(v) => v,
                _ => return None,
            }
        };
        let Declaration::Variable(decl) = symbol.declaration else {
            return None;
        };
        let parent = tree.parent(decl)?;
        if tree.text(parent, "kind") != Some("const")
            || tree.flag(parent, "declare")
            || tree.span(decl).end > tree.span(node).start
        {
            return None;
        }
        if tree
            .ancestors(decl)
            .any(|node| tree.kind(node) == "ExportNamedDeclaration")
        {
            return None;
        }
        let case = |node| tree.ancestors(node).find(|&p| tree.kind(p) == "SwitchCase");
        if case(decl).is_some() && case(decl) != case(node) {
            return None;
        }
        let init = tree.inner(tree.child(decl, "init")?);
        if tree.kind(init) != "ObjectExpression" || !readonly_members(tree, symbol) {
            return None;
        }
        let mut selected = None;
        let mut own = HashSet::new();
        for property in tree.list(init, "properties").into_iter().flatten() {
            if tree.kind(property) != "Property"
                || tree.flag(property, "computed")
                || tree.flag(property, "method")
                || tree.text(property, "kind") != Some("init")
            {
                return None;
            }
            let name = crate::lang::property_name(tree, property)?;
            if matches!(name, "__proto__" | "toString" | "valueOf") {
                return None;
            }
            let value = self.read(parsed, path, tree.child(property, "value")?, bound)?;
            own.insert(name);
            if name == key {
                selected = Some(value);
            }
        }
        if symbol.references.iter().any(|reference| {
            reference
                .node
                .and_then(|id| tree.parent(id))
                .and_then(|member| crate::lang::member_name(tree, member))
                .is_none_or(|name| !own.contains(name))
        }) {
            return None;
        }
        selected
    }

    /// Cached named export values never borrow node identifiers from a different parsed tree.
    fn exported(&mut self, file: &Path, name: &str) -> Option<Scalar> {
        let key = (file.to_owned(), name.to_owned());
        if let Some(value) = self.setting.exports.values.borrow().get(&key) {
            return value.clone();
        }
        if self.active.len() >= 16 || !self.active.insert(key.clone()) {
            return None;
        }
        let confined = crate::lang::confinement_refusals();
        let result = self.export_uncached(file, name);
        self.active.remove(&key);
        if result.is_some() && crate::lang::confinement_refusals() == confined {
            self.setting
                .exports
                .values
                .borrow_mut()
                .insert(key, result.clone());
        }
        result
    }

    /// Follows explicit runtime exports and local aliases, refusing star ambiguity and TS erasure.
    fn export_uncached(&mut self, file: &Path, name: &str) -> Option<Scalar> {
        let source = self.setting.exports.sources.read(file)?;
        self.bytes = self.bytes.saturating_add(source.len());
        if self.bytes > MAX_SOURCE_BYTES {
            return None;
        }
        let parsed = crate::lang::parse(file, &source).ok()?;
        let view = View {
            tree: &parsed.tree,
            scopes: &parsed.scopes,
        };
        let tree = &parsed.tree;
        for statement in tree.list(tree.root(), "body").into_iter().flatten() {
            if tree.kind(statement) == "ExportDefaultDeclaration" && name == "default" {
                return self.read(
                    view,
                    file,
                    tree.child(statement, "declaration")?,
                    &HashMap::new(),
                );
            }
            if tree.kind(statement) != "ExportNamedDeclaration"
                || tree.text(statement, "exportKind") == Some("type")
            {
                continue;
            }
            for specifier in tree.list(statement, "specifiers").into_iter().flatten() {
                if tree.text(specifier, "exportKind") == Some("type")
                    || crate::lang::export_name(tree, tree.child(specifier, "exported")?) != name
                {
                    continue;
                }
                let local = crate::lang::export_name(tree, tree.child(specifier, "local")?);
                if let Some(source) = tree
                    .child(statement, "source")
                    .and_then(|s| tree.text(s, "value"))
                {
                    let next =
                        self.setting
                            .exports
                            .sources
                            .resolve(file, source, self.setting.root)?;
                    return self.exported(&next, &local);
                }
                return self.binding(
                    view,
                    file,
                    parsed.scopes.top_level(&local)?,
                    None,
                    &HashMap::new(),
                );
            }
            if let Some(declaration) = tree.child(statement, "declaration") {
                for declarator in tree.list(declaration, "declarations").into_iter().flatten() {
                    let id = tree.child(declarator, "id")?;
                    if tree.kind(id) == "Identifier" && tree.text(id, "name") == Some(name) {
                        return self.binding(
                            view,
                            file,
                            parsed.scopes.symbol_at(tree.span(id).start)?,
                            None,
                            &HashMap::new(),
                        );
                    }
                }
            }
        }
        None
    }
}
