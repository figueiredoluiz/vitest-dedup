use super::*;
use crate::frontend::js_text::string_text;

/// Extracts, from a file's full source text, each top-level import's local binding name mapped to
/// `(identity component, name on the exporting side)`. `file` is the path of the file being
/// fingerprinted -- passed to [`resolver::resolve`](crate::frontend::resolver::resolve) so a relative or aliased specifier resolves
/// against the file that actually wrote it, and used (via its parent directory) to anchor the
/// unresolved fallback -- see [`normalize_relative_specifier`] and the module doc's discussion of
/// why an unanchored fallback is a proved cross-file false-positive.
///
/// The identity component is `resolved:<canonical path>` when [`resolver::resolve`](crate::frontend::resolver::resolve) finds a real
/// file, or `unresolved:<anchored specifier>` when it does not; see the module doc for why the two
/// are prefixed distinctly and can never collide.
///
/// `Semantic` does not retain this once binding is resolved -- only the AST does, via
/// `ImportDeclaration::source` -- and `fingerprint`'s signature is never handed the `Program`,
/// only `body`/`semantic`/`source`/`file`. So this re-parses `source` with its own throwaway
/// `Allocator` to read straight off the top-level `ImportDeclaration`s, which needs no semantic
/// analysis of its own (imports are always top-level, never nested, so a shallow scan of
/// `program.body` is sufficient). Extraction does not use this re-parse: it reads the map off its
/// own real-type parse ([`with_file_import_origins`]), shared by every body fingerprinted from that
/// file, and helper files use their own parse too. Only callers outside an extraction scope still
/// compute it here.
///
/// `SourceType::tsx()` is used unconditionally as the most permissive superset available (this
/// module is never told the real file extension). A file using the legacy `<T>expr` TypeScript
/// cast syntax -- ambiguous with JSX and therefore rejected in `.tsx` mode -- degrades to an empty
/// map, same as any other parse failure; that only costs import-specifier precision; see
/// `free_identity`'s fallback.
pub(crate) fn import_origins(source: &str, file: &Path) -> ImportOrigins {
    if crate::frontend::extract::parse_refusal(source).is_some() {
        return HashMap::new();
    }
    #[cfg(test)]
    IMPORT_ORIGIN_PARSES.with(|count| count.set(count.get() + 1));
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::tsx()).parse();
    if parsed.fatal_error {
        return HashMap::new();
    }
    import_origins_from_program(&parsed.program, file)
}

/// `program`'s top-level import declarations that bind values: `import type` declarations are
/// erased by TypeScript and never reach runtime, so every import table skips them.
pub(crate) fn value_import_declarations<'p, 'a>(
    program: &'p oxc_ast::ast::Program<'a>,
) -> impl Iterator<Item = &'p oxc_ast::ast::ImportDeclaration<'a>> + 'p {
    program.body.iter().filter_map(|statement| match statement {
        Statement::ImportDeclaration(import)
            if !matches!(import.import_kind, oxc_ast::ast::ImportOrExportKind::Type) =>
        {
            Some(&**import)
        }
        _ => None,
    })
}

/// The import map of an already-parsed `program` written at `file`: each top-level, non-type
/// import's local binding name mapped to its origin (see [`import_origins`] for the identity
/// scheme). Needs no semantic analysis, because imports are always top-level.
pub(crate) fn import_origins_from_program(
    program: &oxc_ast::ast::Program<'_>,
    file: &Path,
) -> ImportOrigins {
    let mut origins = HashMap::new();
    for import in value_import_declarations(program) {
        let Some(specifiers) = &import.specifiers else {
            continue;
        };
        let module: &str = &string_text(&import.source);
        let origin = specifier_origin(file, module);
        for specifier in specifiers {
            let (local, exported) = match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                    if matches!(
                        specifier.import_kind,
                        oxc_ast::ast::ImportOrExportKind::Type
                    ) {
                        continue;
                    }
                    (&specifier.local, module_export_name(&specifier.imported))
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                    (&specifier.local, "default".to_owned())
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                    (&specifier.local, "*".to_owned())
                }
            };
            origins.insert(
                local.name.as_str().to_owned(),
                ImportOrigin {
                    component: origin.component.clone(),
                    exported,
                    specifier: module.to_owned(),
                    resolved: origin.resolved.clone(),
                },
            );
        }
    }
    origins
}

/// Identifies one file's text for [`FILE_IMPORTS`]: the borrowed source's address and length plus
/// the path. The scope that installs an entry borrows that source for its whole lifetime, so the
/// address cannot be reused by other text while the entry is visible.
#[derive(PartialEq, Eq)]
struct FileKey {
    source: usize,
    len: usize,
    path: PathBuf,
}

impl FileKey {
    /// Builds the key for `source` as written at `file`.
    fn of(source: &str, file: &Path) -> Self {
        Self {
            source: source.as_ptr() as usize,
            len: source.len(),
            path: file.to_path_buf(),
        }
    }
}

thread_local! {
    /// The import origins of the file being extracted on this thread, computed once for it by
    /// [`with_file_import_origins`]. Without this, every fingerprinted body re-parsed the whole file
    /// and re-resolved its imports, which made a file's cost quadratic in its test count.
    static FILE_IMPORTS: std::cell::RefCell<Option<(FileKey, std::rc::Rc<ImportOrigins>)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// Test-only count of [`import_origins`] parses on this thread.
    static IMPORT_ORIGIN_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Returns the number of [`import_origins`] parses this thread has performed.
#[cfg(test)]
pub(crate) fn import_origin_parses() -> usize {
    IMPORT_ORIGIN_PARSES.with(std::cell::Cell::get)
}

/// Runs `walk` with `file`'s import origins, read once from `program` (the caller's parse of
/// `source`), shared by every body fingerprinted from that same `source` during the walk; the
/// previous state is restored afterwards, including on unwind.
pub(crate) fn with_file_import_origins<R>(
    source: &str,
    file: &Path,
    program: &oxc_ast::ast::Program<'_>,
    walk: impl FnOnce() -> R,
) -> R {
    /// Puts back the entry that was visible before this scope.
    struct Restore(Option<(FileKey, std::rc::Rc<ImportOrigins>)>);
    impl Drop for Restore {
        /// Reinstalls the saved entry when the scope ends, normally or by unwinding.
        fn drop(&mut self) {
            let previous = self.0.take();
            FILE_IMPORTS.with(|slot| *slot.borrow_mut() = previous);
        }
    }
    // Read off the caller's own parse, made with the file's real source type: a second TSX parse
    // would reject valid `.ts` syntax (a legacy `<T>value` cast, a generic arrow) and silently
    // empty the map.
    let entry = (
        FileKey::of(source, file),
        std::rc::Rc::new(import_origins_from_program(program, file)),
    );
    let _restore = Restore(FILE_IMPORTS.with(|slot| slot.replace(Some(entry))));
    walk()
}

/// Returns `file`'s import origins: the shared copy when `source` is the file being walked, else a
/// fresh computation (helper files, direct callers outside an extraction scope).
pub(crate) fn file_import_origins(source: &str, file: &Path) -> std::rc::Rc<ImportOrigins> {
    let key = FileKey::of(source, file);
    FILE_IMPORTS
        .with(|slot| {
            slot.borrow()
                .as_ref()
                .filter(|(installed, _)| *installed == key)
                .map(|(_, origins)| std::rc::Rc::clone(origins))
        })
        .unwrap_or_else(|| std::rc::Rc::new(import_origins(source, file)))
}

/// Resolves `specifier`, as written in `file`, to a fingerprint-safe identity component:
/// `resolved:<canonical path>` when [`resolver::resolve`](crate::frontend::resolver::resolve) finds a real file on disk, or
/// `unresolved:<anchored specifier>` when it does not -- the two are prefixed distinctly so a
/// failed resolution can never collide with a successful one, regardless of what path string a
/// resolution could ever produce.
///
/// This is the one place a file-relative specifier is turned into something safe to compare
/// *across* files, and every caller that puts repository text from a `vi.mock`/`import` argument
/// into a cross-file-reaching identity must go through it rather than storing the argument's raw
/// text -- see [`import_origins`] (free-import identity, the original caller) and
/// `extract::collect_mocked_modules` (`MockedModule::specifier`, the same shape of bug found a
/// sixth time: `vi.mock('./dep')` is file-relative exactly like an `import` specifier is, but was
/// being stored as the raw argument text with no anchoring at all, so the same relative text in
/// two different directories named two different modules yet compared equal).
///
/// A bare/package specifier (`vitest`, `@scope/pkg`) that fails to resolve (no `node_modules`, no
/// matching tsconfig `paths` alias) falls back to the same directory-independent text it always
/// used: two files that both fail to resolve the same package name are not proven to differ, so
/// there is nothing to anchor. An absolute-style specifier (a bare leading `/`) is left as-is too,
/// for the same reason. A successful resolution of a bare specifier is naturally file-independent
/// already (it names the same real file -- typically the same installed package -- regardless of
/// which file in the project imports it), so this function needs no special case for "bare" versus
/// "relative" on the `resolved:` path at all; only the `unresolved:` fallback needs to tell them
/// apart, via [`normalize_relative_specifier`].
pub(crate) fn specifier_identity(file: &Path, specifier: &str) -> String {
    specifier_origin(file, specifier).component
}

/// [`specifier_identity`], keeping the resolution's by-products the identity string itself
/// discards: the raw specifier and the resolved path, which [`import_origins`] stores on every
/// binding so the inlining path (`try_inline_imported_helper`) can reach the helper's file and
/// refuse the framework by its raw specifier.
fn specifier_origin(file: &Path, specifier: &str) -> ImportOrigin {
    match crate::frontend::resolver::resolve(file, specifier) {
        crate::frontend::resolver::Resolved::Module(resolved_path) => ImportOrigin {
            component: format!("resolved:{}", path_key(&resolved_path)),
            exported: String::new(),
            specifier: specifier.to_owned(),
            resolved: Some(resolved_path),
        },
        crate::frontend::resolver::Resolved::Unresolved => {
            let anchored = if specifier.starts_with('.') {
                let importer_dir = file.parent().unwrap_or_else(|| Path::new(""));
                normalize_relative_specifier(importer_dir, specifier)
            } else {
                specifier.to_owned()
            };
            ImportOrigin {
                component: format!("unresolved:{anchored}"),
                exported: String::new(),
                specifier: specifier.to_owned(),
                resolved: None,
            }
        }
    }
}

/// The text of a `ModuleExportName` -- the `foo` of `import { foo as bar }` in whichever of its
/// three syntactic shapes it was written -- as the exporting side spelled it. Shared with
/// `helpers.rs`, whose export lookup keys on exactly these names.
pub(crate) fn module_export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(id) => id.name.as_str().to_owned(),
        ModuleExportName::IdentifierReference(id) => id.name.as_str().to_owned(),
        ModuleExportName::StringLiteral(literal) => string_text(literal).into_owned(),
    }
}

/// Renders a filesystem path as a stable, `/`-separated string, for use as an identity component
/// (`free:module:<file>:name`, `free:outer:<file>:name`) and as the starting point for
/// [`normalize_relative_specifier`]. `/`-joining rather than using the path's native separator
/// keeps the result identical regardless of the host platform, which matters because this string
/// is baked into a fingerprint that two files (or two runs) are compared by.
///
/// `RootDir`/`CurDir`/`ParentDir` components contribute no segment of their own: a leading
/// `RootDir` would only add a spurious empty segment (this function does not itself prefix a
/// leading `/`, since only relative composition in `normalize_relative_specifier` needs the
/// segments as a plain stack), and `CurDir`/`ParentDir` should not appear in an already-absolute
/// path in the first place. `Prefix` (a Windows drive letter or UNC root) is kept, so two files on
/// different drives never collide.
fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().into_owned()),
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::RootDir | Component::CurDir | Component::ParentDir => None,
        })
        .collect()
}

/// Join normalized path segments into the file identity used by fingerprint resolution.
pub(crate) fn path_key(path: &Path) -> String {
    path_segments(path).join("/")
}

/// Resolves a relative module specifier (`./x`, `../x`) against the importing file's directory
/// into an anchored, `/`-separated path -- lexically, by manipulating path segments as text, with
/// no filesystem access and no attempt at extension or index-file resolution.
///
/// This is no longer the primary way a relative specifier's identity is computed -- that is now
/// [`resolver::resolve`](crate::frontend::resolver::resolve), which does real filesystem resolution. This function survives
/// as the *unresolved* fallback (see [`import_origins`]): when `resolver::resolve` cannot find a
/// real file, this still stops two files in different directories that both write the same
/// relative specifier (e.g. `import { helper } from './helpers'`) from being treated as the same
/// import, which an adversarial review proved happens without it, and which resolution failing is
/// not a reason to stop guarding against.
///
/// `..` never pops past an empty stack: an import climbing above the importer's own path is not
/// valid input this function needs to resolve correctly, only safely (never panic, never
/// underflow). `importer_dir` is expected to already be absolute (callers pass a `SourceFile`'s
/// own path), so in practice this never triggers, but a defensive floor costs nothing here.
fn normalize_relative_specifier(importer_dir: &Path, specifier: &str) -> String {
    let mut segments = path_segments(importer_dir);
    for segment in specifier.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other.to_owned()),
        }
    }
    segments.join("/")
}
