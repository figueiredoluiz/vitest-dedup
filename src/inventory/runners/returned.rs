//! Structural returned-object proofs, before any returned callback is admitted.
use super::*;

/// A checked first-party callable that constructs one inert object.
///
/// This proves construction only. Reading a member or invoking a returned closure needs a
/// separate proof of argument binding, initialization order and registration effects.
#[derive(Debug)]
pub struct ReturnedTemplate {
    /// The module containing the resolved callable.
    pub path: PathBuf,
    /// Exact source bytes used by the proof.
    pub source: String,
    /// Owned tree and bindings; all node and symbol IDs belong to this module.
    pub parsed: Parsed,
    /// The synchronous function invoked.
    pub function: NodeId,
    /// Plain immutable positional bindings, in call order.
    pub parameters: Vec<SymbolId>,
    /// The returned object expression, whose callbacks remain uninterpreted.
    pub object: NodeId,
    /// Inert declarations evaluated before selecting the returned object.
    pub setup: Vec<NodeId>,
    /// Actual setup/property proof work consumed from the supplied allowance.
    pub work: usize,
    /// Eagerly evaluated project dependencies checked by export resolution.
    pub dependencies: Vec<PathBuf>,
    /// Export hops to charge to an invocation's shared recursion allowance.
    pub resolution_depth: usize,
    /// Source bytes read by the proof, for a caller's shared source allowance.
    pub dependency_bytes: Vec<(PathBuf, usize)>,
}

/// Resolve a first-party export and prove inert object construction within `budget` work units.
/// Defaults, destructuring and rest parameters retain the ordinary runner reader's refusal.
pub fn returned_template(
    sources: &Sources,
    path: &Path,
    export: &str,
    setting: Setting<'_>,
    budget: usize,
) -> Result<ReturnedTemplate, Refusal> {
    let resolved = resolved_template(sources, path, export, setting)?;
    let parameters = parameters(
        &resolved.parsed,
        resolved.function,
        resolved.found.direct_eval,
    )?;
    let (object, setup, work) =
        returned_object(&resolved.parsed, resolved.function, &parameters, budget)?;
    let mut dependencies: Vec<_> = resolved.dependencies.into_iter().collect();
    dependencies.sort();
    let mut dependency_bytes: Vec<_> = resolved.dependency_bytes.into_iter().collect();
    dependency_bytes.sort();
    Ok(ReturnedTemplate {
        path: resolved.path,
        source: resolved.source,
        parsed: resolved.parsed,
        function: resolved.function,
        parameters,
        object,
        setup,
        work,
        dependencies,
        resolution_depth: resolved.resolution_depth,
        dependency_bytes,
    })
}

/// Keep exhausted work distinct from unsupported construction.
fn refusal(proof: &InertProof<'_>) -> Refusal {
    if proof.limited {
        Refusal::Limit
    } else {
        Refusal::Body
    }
}

/// Read a final object return after inert setup without interpreting any returned closure.
fn returned_object(
    parsed: &Parsed,
    function: NodeId,
    parameters: &[SymbolId],
    budget: usize,
) -> Result<(NodeId, Vec<NodeId>, usize), Refusal> {
    let tree = &parsed.tree;
    let body = tree.child(function, "body").ok_or(Refusal::Body)?;
    if tree.kind(body) != "BlockStatement" {
        return Err(Refusal::Body);
    }
    let statements: Vec<_> = tree
        .list(body, "body")
        .into_iter()
        .flatten()
        .filter(|&node| !tree.erased_declaration(node) && tree.kind(node) != "EmptyStatement")
        .collect();
    if statements.len() > 2000 {
        return Err(Refusal::Limit);
    }
    let (&returned, setup) = statements.split_last().ok_or(Refusal::Body)?;
    let mut proof = InertProof::new(parsed, budget);
    if !setup.iter().all(|&statement| {
        tree.kind(statement) == "FunctionDeclaration"
            || factory_declaration(&mut proof, statement, parameters)
    }) || tree.kind(returned) != "ReturnStatement"
    {
        return Err(refusal(&proof));
    }
    let value = tree.inner(tree.child(returned, "argument").ok_or(Refusal::Body)?);
    let object = match tree.kind(value) {
        "ObjectExpression" => value,
        "Identifier" => {
            let symbol = parsed
                .scopes
                .symbol_at(tree.span(value).start)
                .ok_or(Refusal::Body)?;
            if !proof.immutable(symbol) {
                return Err(refusal(&proof));
            }
            let binding = parsed.scopes.symbol(symbol);
            let Declaration::Variable(declaration) = binding.declaration else {
                return Err(Refusal::Body);
            };
            if tree
                .parent(declaration)
                .and_then(|parent| tree.text(parent, "kind"))
                != Some("const")
                || binding.references.len() != 1
                || binding.references[0].span != tree.span(value)
            {
                return Err(Refusal::Body);
            }
            tree.inner(tree.child(declaration, "init").ok_or(Refusal::Body)?)
        }
        _ => return Err(Refusal::Body),
    };
    if tree.kind(object) != "ObjectExpression" || !proof.value(object, &mut 4096) {
        return Err(refusal(&proof));
    }
    let mut properties = 4096;
    returned_properties(parsed, object, &mut properties)?;
    if !proof.spend(4096 - properties) || proof.limited {
        return Err(refusal(&proof));
    }
    Ok((object, setup.to_vec(), budget - proof.left))
}

/// Const setup copies inert data or a stable parameter/import without invoking user code.
fn factory_declaration(
    proof: &mut InertProof<'_>,
    statement: NodeId,
    parameters: &[SymbolId],
) -> bool {
    let parsed = proof.parsed;
    let tree = &parsed.tree;
    tree.kind(statement) == "VariableDeclaration"
        && tree.text(statement, "kind") == Some("const")
        && tree
            .list(statement, "declarations")
            .into_iter()
            .flatten()
            .all(|declarator| {
                tree.child(declarator, "id")
                    .is_some_and(|id| tree.kind(id) == "Identifier")
                    && tree
                        .child(declarator, "init")
                        .is_some_and(|init| factory_value(proof, init, &mut 4096, parameters))
            })
}

/// Parameter identity uses recorded symbols; same-spelled bindings do not establish a proof.
fn factory_value(
    proof: &mut InertProof<'_>,
    node: NodeId,
    left: &mut usize,
    parameters: &[SymbolId],
) -> bool {
    if proof.value(node, left) {
        return true;
    }
    if *left == 0 || proof.limited || !proof.spend(1) {
        return false;
    }
    *left -= 1;
    let parsed = proof.parsed;
    let tree = &parsed.tree;
    let node = tree.inner(node);
    if tree.kind(node) != "Identifier" {
        return false;
    }
    parsed
        .scopes
        .symbol_at(tree.span(node).start)
        .is_some_and(|symbol| {
            if !proof.unwritten(symbol) {
                return false;
            }
            let binding = parsed.scopes.symbol(symbol);
            match binding.declaration {
                Declaration::Other(_) => {
                    for &parameter in parameters {
                        if !proof.spend(1) {
                            return false;
                        }
                        if parameter == symbol {
                            return true;
                        }
                    }
                    false
                }
                Declaration::Import {
                    import, specifier, ..
                } => {
                    tree.text(import, "importKind") != Some("type")
                        && tree.text(specifier, "importKind") != Some("type")
                }
                _ => false,
            }
        })
}

/// Check already-proved owned values through aliases and arrays, leaving callbacks uninterpreted.
fn returned_properties(parsed: &Parsed, value: NodeId, left: &mut usize) -> Result<(), Refusal> {
    if *left == 0 {
        return Err(Refusal::Limit);
    }
    *left -= 1;
    let tree = &parsed.tree;
    let value = tree.inner(value);
    match tree.kind(value) {
        "Identifier" => {
            let symbol = parsed
                .scopes
                .symbol_at(tree.span(value).start)
                .ok_or(Refusal::Body)?;
            // The construction proof already established immutable const initialization.
            if let Declaration::Variable(declaration) = parsed.scopes.symbol(symbol).declaration {
                let init = tree.child(declaration, "init").ok_or(Refusal::Body)?;
                returned_properties(parsed, init, left)?;
            }
            return Ok(());
        }
        "ArrayExpression" => {
            for element in tree.list(value, "elements").into_iter().flatten() {
                returned_properties(parsed, element, left)?;
            }
            return Ok(());
        }
        "ObjectExpression" => {}
        _ => return Ok(()),
    }
    let mut names = HashSet::new();
    for property in tree.list(value, "properties").into_iter().flatten() {
        if tree.kind(property) != "Property"
            || tree.flag(property, "computed")
            || tree.flag(property, "method")
            || tree.text(property, "kind") != Some("init")
        {
            return Err(Refusal::Body);
        }
        let name = lang::property_name(tree, property).ok_or(Refusal::Body)?;
        if name == "__proto__" || !names.insert(name) {
            return Err(Refusal::Body);
        }
        let value = tree.inner(tree.child(property, "value").ok_or(Refusal::Body)?);
        returned_properties(parsed, value, left)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
