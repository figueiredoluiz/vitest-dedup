//! The names a Nuxt project makes global without an import statement.
//!
//! Nuxt writes `.nuxt/imports.d.ts` when it prepares the project: one `export { a, b as c } from
//! '<module>'` line per source, naming every auto-imported binding and where it comes from.
//! Reading that file answers "what does this unimported name stand for" from the project's own
//! generated facts rather than a guess. A name that is not listed is unknown, and unknown names
//! are never assumed harmless.
//!
//! Only the nearest `.nuxt/imports.d.ts` at or above a file is consulted. A project that was not
//! prepared has none, so nothing resolves -- the missing-prerequisite notice already tells the
//! user to run `nuxt prepare`.

use crate::lang::js_text::string_text;
use oxc_allocator::Allocator;
use oxc_ast::ast::{ImportOrExportKind, Statement};
use oxc_parser::Parser;
use oxc_span::SourceType;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where an auto-imported name comes from: the generated file that lists it and the module
/// specifier as written there (relative to that file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AutoImport {
    pub(crate) importer: PathBuf,
    pub(crate) specifier: String,
    /// The export it has in that module (`default` for `default as name`).
    pub(crate) imported: String,
}

type Table = HashMap<String, AutoImport>;

thread_local! {
    /// One parse per generated file per worker.
    static TABLES: RefCell<HashMap<PathBuf, Option<Table>>> = RefCell::new(HashMap::new());
}

/// Reads one `imports.d.ts` into name -> origin. `None` when reading, admission or parsing fails.
fn read_table(file: &Path) -> Option<Table> {
    let source =
        crate::discovery::read_bounded(file, crate::discovery::MAX_SOURCE_FILE_BYTES).ok()??;
    if super::extract::parse_refusal(&source).is_some() {
        return None;
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, SourceType::ts()).parse();
    if parsed.fatal_error {
        return None;
    }
    let mut table = Table::new();
    for statement in &parsed.program.body {
        let Statement::ExportFromDeclaration(export) = statement else {
            continue;
        };
        if matches!(export.export_kind, ImportOrExportKind::Type) {
            continue;
        }
        for specifier in &export.specifiers {
            if matches!(specifier.export_kind, ImportOrExportKind::Type) {
                continue;
            }
            table.insert(
                specifier.exported.name().to_string(),
                AutoImport {
                    importer: file.to_path_buf(),
                    specifier: string_text(&export.source).into_owned(),
                    imported: specifier.local.name().to_string(),
                },
            );
        }
    }
    Some(table)
}

/// The nearest generated `imports.d.ts` at or above `file`.
fn generated_file(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .map(|directory| directory.join(".nuxt/imports.d.ts"))
        .find(|candidate| candidate.is_file())
}

/// What `name` stands for in the project that `file` belongs to, when that project has a
/// generated auto-import list that names it.
pub(crate) fn lookup(file: &Path, name: &str) -> Option<AutoImport> {
    let generated = generated_file(file)?;
    TABLES.with(|tables| {
        let mut tables = tables.borrow_mut();
        tables
            .entry(generated.clone())
            .or_insert_with(|| read_table(&generated))
            .as_ref()?
            .get(name)
            .cloned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Byte admission includes the exact ceiling, but never accepts the next byte.
    #[test]
    fn table_source_size_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("imports.d.ts");
        let mut source = "export { clock } from 'vue';".to_owned();
        source.extend(std::iter::repeat_n(
            ' ',
            crate::discovery::MAX_SOURCE_FILE_BYTES as usize - source.len(),
        ));
        fs::write(&file, &source).unwrap();
        assert!(read_table(&file).unwrap().contains_key("clock"));
        source.push(' ');
        fs::write(&file, source).unwrap();
        assert!(read_table(&file).is_none());
    }

    /// Raw syntax admission remains independent of bracket masking, even inside a comment.
    #[test]
    fn table_raw_syntax_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("imports.d.ts");
        // The comment delimiters contribute four raw units; its contents add the rest.
        let source = format!("/*{}*/", "!".repeat(36_864 - 4));
        fs::write(&file, &source).unwrap();
        assert!(read_table(&file).is_some());
        fs::write(&file, source.replace("*/", "!*/")).unwrap();
        assert!(read_table(&file).is_none());
    }

    /// Read errors and invalid UTF-8 are refused, and a failed lookup remains failed in its cache.
    #[test]
    fn unreadable_tables_and_cached_refusals_resolve_nothing() {
        let directory = tempfile::tempdir().unwrap();
        assert!(read_table(directory.path()).is_none());
        let generated = directory.path().join(".nuxt/imports.d.ts");
        fs::create_dir_all(generated.parent().unwrap()).unwrap();
        fs::write(&generated, [0xff]).unwrap();
        let file = directory.path().join("test.ts");
        assert!(lookup(&file, "clock").is_none());
        fs::write(&generated, "export { clock } from 'vue';").unwrap();
        assert!(lookup(&file, "clock").is_none());
    }

    #[test]
    fn names_resolve_through_the_nearest_generated_list() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(".nuxt")).unwrap();
        fs::write(
            root.join(".nuxt/imports.d.ts"),
            "export { isVue2 } from 'vue-demi';\n\
             export { default as useStorage, Storage } from '../composables/useStorage';\n\
             export type { Ref } from 'vue';\n",
        )
        .unwrap();
        let file = root.join("composables/nested/use.ts");
        let found = lookup(&file, "useStorage").unwrap();
        assert_eq!(found.specifier, "../composables/useStorage");
        assert_eq!(found.importer, root.join(".nuxt/imports.d.ts"));
        assert_eq!(lookup(&file, "isVue2").unwrap().specifier, "vue-demi");
        assert!(
            lookup(&file, "Ref").is_none(),
            "type exports are not values"
        );
        assert!(lookup(&file, "unlisted").is_none());
    }

    #[test]
    fn no_generated_list_means_nothing_resolves() {
        let directory = tempfile::tempdir().unwrap();
        assert!(lookup(&directory.path().join("a.ts"), "useX").is_none());
    }

    #[test]
    fn an_unparseable_list_resolves_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(".nuxt")).unwrap();
        fs::write(root.join(".nuxt/imports.d.ts"), "export { from ;;; {{{").unwrap();
        assert!(lookup(&root.join("a.ts"), "useX").is_none());
    }
}
