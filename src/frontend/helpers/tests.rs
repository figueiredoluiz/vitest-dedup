//! Tests for imported-helper resolution, export lookup and the helper source cache.

use super::export_contract::{resolve_export, ExportRefusal};
use super::*;

/// Returns how many helper files this thread has read so far.
fn helper_reads() -> usize {
    HELPER_READS.with(std::cell::Cell::get)
}

/// Writes `text` to `path` and sets its modification time to `modified`.
fn write_at(path: &Path, text: &str, modified: std::time::SystemTime) {
    std::fs::write(path, text).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

/// A modification time old enough for the cache to trust.
fn settled_time() -> std::time::SystemTime {
    std::time::SystemTime::now() - std::time::Duration::from_secs(3600)
}

/// Returns how many helper files this thread has parsed so far.
fn helper_parses() -> usize {
    HELPER_PARSES.with(std::cell::Cell::get)
}

/// The export lookup behind every inline decision is memoized: twenty lookups of one export parse
/// the helper once.
#[test]
fn an_export_lookup_parses_once_across_repeated_calls() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    write_at(
        &helper,
        "export function add(a, b) { return a + b; }",
        settled_time(),
    );
    let before = helper_parses();
    for _ in 0..20 {
        assert_eq!(locate(&helper, "add"), Some(2));
    }
    assert_eq!(
        helper_parses() - before,
        1,
        "20 lookups of one unchanged export"
    );
}

/// A memoized lookup depends on every file its walk read: rewriting the re-export target, not the
/// file that was asked about, still forces a fresh walk.
#[test]
fn a_rewritten_reexport_target_is_walked_again() {
    let dir = tempfile::tempdir().unwrap();
    let barrel = dir.path().join("index.ts");
    let target = dir.path().join("impl.ts");
    let modified = settled_time();
    write_at(&barrel, "export { add } from './impl'", modified);
    write_at(&target, "export function add(a) { return a; }", modified);
    assert_eq!(locate(&barrel, "add"), Some(1));
    write_at(
        &target,
        "export function add(a, b) { return a + b; }",
        modified,
    );
    assert_eq!(
        locate(&barrel, "add"),
        Some(2),
        "the rewritten target's arity"
    );
}

/// A lookup that read a recently modified file is not memoized, and reads it once per walk.
#[test]
fn a_lookup_over_an_unsettled_file_is_not_memoized() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    write_at(
        &helper,
        "export function add(a, b) { return a + b; }",
        std::time::SystemTime::now(),
    );
    let (parses, reads) = (helper_parses(), helper_reads());
    assert_eq!(locate(&helper, "add"), Some(2));
    assert_eq!(locate(&helper, "add"), Some(2));
    assert_eq!(
        helper_parses() - parses,
        2,
        "each lookup walks the unsettled file"
    );
    assert_eq!(helper_reads() - reads, 2, "one read per walk");
}

/// A re-export whose target is missing is walked again once the target exists. The second lookup
/// runs on a new thread, as a later file's extraction does, since resolution is memoized per
/// worker.
#[test]
fn a_reexport_to_a_created_target_is_walked_again() {
    let dir = tempfile::tempdir().unwrap();
    let barrel = dir.path().join("index.ts");
    let modified = settled_time();
    write_at(&barrel, "export { add } from './impl'", modified);
    assert_eq!(locate(&barrel, "add"), None);
    write_at(
        &dir.path().join("impl.ts"),
        "export function add(a) { return a; }",
        modified,
    );
    let created = std::thread::spawn(move || locate(&barrel, "add"))
        .join()
        .unwrap();
    assert_eq!(created, Some(1), "the created target");
}

/// One helper file is read once however many call sites ask about it (P4): every inline
/// decision used to re-read and re-scan the file, which dominated helper-heavy runs.
#[test]
fn a_helper_file_is_read_once_across_repeated_lookups() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    write_at(
        &helper,
        "export function add(a, b) { return a + b; }",
        settled_time(),
    );
    let before = helper_reads();
    for _ in 0..20 {
        assert_eq!(locate(&helper, "add"), Some(2));
    }
    assert_eq!(
        helper_reads() - before,
        1,
        "20 lookups of one unchanged helper"
    );
}

/// A helper rewritten in place is read again, even when the rewrite keeps the old
/// modification time: the length alone tells the versions apart.
#[test]
fn a_rewritten_helper_is_read_again() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    let modified = settled_time();
    write_at(&helper, "export function add(a) { return a; }", modified);
    assert_eq!(locate(&helper, "add"), Some(1));
    write_at(
        &helper,
        "export function add(a, b) { return a + b; }",
        modified,
    );
    assert_eq!(
        locate(&helper, "add"),
        Some(2),
        "the rewritten helper's arity"
    );
}

/// A recently modified file is never cached: on a coarse-timestamp filesystem a same-length
/// rewrite inside one tick keeps the old stamp, so only a re-read sees the new text.
#[test]
fn a_recently_modified_helper_is_read_afresh() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    let tick = std::time::SystemTime::now();
    let two = "export function add(a, b) { return a + b; }";
    let one = "export function add(a) { return a + 1001; }";
    assert_eq!(two.len(), one.len(), "the rewrite keeps the length");
    write_at(&helper, two, tick);
    assert_eq!(locate(&helper, "add"), Some(2));
    write_at(&helper, one, tick);
    assert_eq!(
        locate(&helper, "add"),
        Some(1),
        "same length, same stamp, new text"
    );
}

/// A read error is not remembered: a helper that becomes readable again is admitted even
/// though its length and modification time never changed (`chmod` touches neither).
#[cfg(unix)]
#[test]
fn a_read_error_is_retried_rather_than_cached() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    write_at(
        &helper,
        "export function add(a, b) { return a + b; }",
        settled_time(),
    );
    let set_mode = |mode| {
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    set_mode(0o000);
    if std::fs::read(&helper).is_ok() {
        set_mode(0o644);
        return; // running as root: permissions cannot make the file unreadable here
    }
    assert_eq!(
        locate(&helper, "add"),
        None,
        "an unreadable helper is refused"
    );
    set_mode(0o644);
    assert_eq!(
        locate(&helper, "add"),
        Some(2),
        "and admitted once readable again"
    );
}

/// The cap bounds held bytes, and a version the cap rejects also evicts the path's older,
/// unservable version instead of leaving it counted.
#[test]
fn the_cache_cap_evicts_a_replaced_version_it_cannot_hold() {
    let stamp = |len| FileStamp {
        len,
        modified: std::time::SystemTime::UNIX_EPOCH,
    };
    let text = |s: &str| Some(std::sync::Arc::<str>::from(s));
    let mut sources = HelperSources::default();
    let a = Path::new("/a.ts");
    sources.remember(a, stamp(6), text("abcdef"), 10);
    assert_eq!(sources.bytes, 6);
    sources.remember(a, stamp(12), text("abcdefghijkl"), 10);
    assert!(!sources.entries.contains_key(a), "the old version is gone");
    assert_eq!(sources.bytes, 0, "and so are its bytes");
    sources.remember(Path::new("/b.ts"), stamp(8), text("abcdefgh"), 10);
    sources.remember(Path::new("/c.ts"), stamp(3), text("abc"), 10);
    assert_eq!(sources.bytes, 8, "a file past the cap is not held");
    sources.remember(Path::new("/d.ts"), stamp(1), None, 10);
    assert!(
        sources.entries.contains_key(Path::new("/d.ts")),
        "refusals cost no bytes"
    );
}

/// A refused helper stays refused and is not re-read while unchanged; an installed package
/// is never read at all.
#[test]
fn refusals_are_cached_and_dependencies_are_never_read() {
    let dir = tempfile::tempdir().unwrap();
    let refused = dir.path().join("refused.ts");
    let deep = "(".repeat(9_000);
    write_at(
        &refused,
        &format!("export function f(a) {{ return {deep}a; }}"),
        settled_time(),
    );
    let before = helper_reads();
    assert_eq!(locate(&refused, "f"), None);
    assert_eq!(locate(&refused, "f"), None);
    assert_eq!(
        helper_reads() - before,
        1,
        "a refusal is decided once per file version"
    );

    let package = dir.path().join("node_modules").join("pkg");
    std::fs::create_dir_all(&package).unwrap();
    let dependency = package.join("index.ts");
    std::fs::write(&dependency, "export function g(a) { return a; }").unwrap();
    let before = helper_reads();
    assert_eq!(locate(&dependency, "g"), None);
    assert_eq!(
        helper_reads() - before,
        0,
        "an installed package is never read"
    );
}

/// An unparseable helper file refuses the whole inline (under-match), never panics --
/// the product's degrade-visibly rule.
#[test]
fn a_parse_failure_is_a_refusal_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    std::fs::write(&helper, "export function broken( {").unwrap();
    assert_eq!(
        locate(&helper, "broken"),
        None,
        "an unparseable helper file must refuse, never panic"
    );
}

/// A defaulted parameter's value depends on the caller's arity, so the whole export is
/// refused rather than partially substituted.
#[test]
fn default_parameters_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    std::fs::write(
        &helper,
        "export function greet(name = 'x') { return name; }",
    )
    .unwrap();
    assert_eq!(
        locate(&helper, "greet"),
        None,
        "a defaulted parameter has no argument to substitute; the whole export is refused"
    );
}

/// A rest parameter binds no single argument; the export is refused wholesale.
#[test]
fn rest_parameters_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    std::fs::write(&helper, "export function log(...parts) { return parts; }").unwrap();
    assert_eq!(locate(&helper, "log"), None);
}

/// A destructured parameter binds no single symbol to substitute; refused wholesale.
#[test]
fn destructured_parameters_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    std::fs::write(&helper, "export function read({ name }) { return name; }").unwrap();
    assert_eq!(locate(&helper, "read"), None);
}

/// A `let` export can be reassigned by its own module between calls, so its initializer
/// at declaration time proves nothing about any later call.
#[test]
fn let_exports_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    std::fs::write(
        &helper,
        "export let run = function (name) { return name; };",
    )
    .unwrap();
    assert_eq!(locate(&helper, "run"), None);
}

/// `export const name = <arrow>` is an admitted shape; its parameter count is the arity.
#[test]
fn const_arrows_are_admitted_and_counted() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    std::fs::write(
        &helper,
        "export const run = (name, greeting) => greeting + name;",
    )
    .unwrap();
    assert_eq!(locate(&helper, "run"), Some(2));
}

/// `export default function` is admitted under the exported name `default`.
#[test]
fn default_exported_functions_are_admitted() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    std::fs::write(&helper, "export default function (label) { return label; }").unwrap();
    assert_eq!(locate(&helper, "default"), Some(1));
}

/// `export { local as exported }` resolves to the local declaration in the same file.
#[test]
fn same_file_aliases_resolve_to_the_local_declaration() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helpers.ts");
    std::fs::write(
        &helper,
        "function check(label) { return label; }\nexport { check as verify };",
    )
    .unwrap();
    assert_eq!(locate(&helper, "verify"), Some(1));
}

/// Named re-export chains are followed (local names may change mid-chain), and a cycle
/// in the chain terminates in a refusal.
#[test]
fn reexport_chains_are_followed_and_cycles_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    // A two-hop chain with the name changing on the middle hop: index exports `go`, which is
    // mid's `go`, which is inner's `run`.
    std::fs::write(dir.path().join("index.ts"), "export { go } from './mid';").unwrap();
    std::fs::write(
        dir.path().join("mid.ts"),
        "export { run as go } from './inner';",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("inner.ts"),
        "export function run(label) { return label; }",
    )
    .unwrap();
    assert_eq!(locate(&dir.path().join("index.ts"), "go"), Some(1));

    // A cycle: a re-exports b, b re-exports a. Following must terminate in a refusal.
    let cycle = cycle_dir();
    assert_eq!(locate(&cycle.path().join("a.ts"), "spin"), None);
}

/// A resolved path inside `node_modules` is refused before the file is read, both for
/// a direct import and for a first-party barrel's re-export hop into the package.
#[test]
fn an_installed_package_is_refused_before_it_is_read() {
    let dir = package_and_barrel();
    assert_eq!(
        locate(
            &dir.path()
                .join("node_modules")
                .join("a-pkg")
                .join("index.js"),
            "run"
        ),
        None,
        "a direct hit inside node_modules is refused"
    );
    assert_eq!(
        locate(&dir.path().join("barrel.ts"), "run"),
        None,
        "a re-export hop into an installed package is refused"
    );
}

/// A helper file that cannot be read is a refusal, never a panic.
#[test]
fn a_missing_file_is_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(locate(&dir.path().join("absent.ts"), "run"), None);
}

// -- the export walk's refusals, one case per shape ----------------------------------------

/// A temporary directory holding `files` as (name, source) pairs.
fn helper_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, source) in files {
        std::fs::write(dir.path().join(name), source).unwrap();
    }
    dir
}

/// `length + 1` files `r0.ts`..: each re-exports `go` from the next, the last defines it.
fn reexport_chain(length: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..length {
        let source = format!("export {{ go }} from './r{}';", index + 1);
        std::fs::write(dir.path().join(format!("r{index}.ts")), source).unwrap();
    }
    let last = dir.path().join(format!("r{length}.ts"));
    std::fs::write(last, "export function go(a) { return a; }").unwrap();
    dir
}

/// A directory holding an installed package whose entry exports `run`, plus a first-party
/// `barrel.ts` re-exporting it: the dependency-refusal shape, shared by the inline walks'
/// tests and the export contract's pins.
fn package_and_barrel() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("node_modules").join("a-pkg");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"a-pkg","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        package.join("index.js"),
        "export function run(label) { return label; }",
    )
    .unwrap();
    std::fs::write(dir.path().join("barrel.ts"), "export { run } from 'a-pkg';").unwrap();
    dir
}

/// Two files re-exporting one name to each other: the export-cycle shape.
fn cycle_dir() -> tempfile::TempDir {
    helper_dir(&[
        ("a.ts", "export { spin } from './b';"),
        ("b.ts", "export { spin } from './a';"),
    ])
}

/// Builds a resolved import origin for `exported` from the helper at `path`.
fn origin_at(path: PathBuf, exported: &str) -> ImportOrigin {
    ImportOrigin {
        component: String::new(),
        exported: exported.to_owned(),
        specifier: "./helper".to_owned(),
        resolved: Some(path),
    }
}

/// Every export shape the walk must step over, refuse, or admit, in one file each. The
/// positive rows are the controls: a walk that refused everything would pass the refusals.
#[test]
fn the_export_walk_steps_over_unrelated_statements_and_refuses_unadmitted_shapes() {
    let cases: &[(&str, &str, &str, Option<usize>)] = &[
        (
            "class before the helper",
            "export class Box {}\nexport function run(a) { return a; }",
            "run",
            Some(1),
        ),
        (
            "a class is not a callable",
            "export class Box {}",
            "Box",
            None,
        ),
        (
            "alias skips its siblings",
            "function a(x) { return x; }\nfunction b(x, y) { return y; }\nexport { a, b as c };",
            "c",
            Some(2),
        ),
        (
            "default skipped for a named export",
            "export default function (x) { return x; }\nexport function run(a, b) { return a; }",
            "run",
            Some(2),
        ),
        (
            "default arrow",
            "export default (label) => label;",
            "default",
            Some(1),
        ),
        (
            "default non-callable",
            "export default 42;",
            "default",
            None,
        ),
        (
            "const function expression",
            "export const run = function (a, b) { return a; };",
            "run",
            Some(2),
        ),
        (
            "const under another name",
            "export const other = (a) => a;",
            "run",
            None,
        ),
        (
            "const destructured",
            "export const { run } = helpers;",
            "run",
            None,
        ),
        ("const non-callable", "export const run = 5;", "run", None),
        (
            "alias to a const arrow",
            "const check = (label) => label;\nclass Unrelated {}\nexport { check as verify };",
            "verify",
            Some(1),
        ),
        (
            "alias to nothing",
            "export { missing as verify };",
            "verify",
            None,
        ),
        (
            "overload signature without a body",
            "export function over(a: string): void;",
            "over",
            None,
        ),
    ];
    for (label, source, export, expected) in cases {
        let dir = helper_dir(&[("helper.ts", source)]);
        assert_eq!(
            locate(&dir.path().join("helper.ts"), export),
            *expected,
            "{label}: `{source}`"
        );
    }
}

/// A re-export hop the walk cannot follow ends in a refusal, and one it can follow still
/// resolves: a sibling specifier is skipped, an unresolvable specifier refuses, and a chain
/// longer than the hop cap refuses while a shorter one resolves.
#[test]
fn reexport_hops_refuse_what_they_cannot_follow() {
    let dir = helper_dir(&[
        ("multi.ts", "export { a, b } from './inner';"),
        ("inner.ts", "export function b(x) { return x; }"),
        ("dangling.ts", "export { x } from './nowhere';"),
    ]);
    assert_eq!(
        locate(&dir.path().join("multi.ts"), "b"),
        Some(1),
        "the sibling `a` is skipped"
    );
    assert_eq!(
        locate(&dir.path().join("dangling.ts"), "x"),
        None,
        "an unresolvable hop refuses"
    );

    let chain = reexport_chain(MAX_REEXPORT_HOPS + 2);
    assert_eq!(
        locate(&chain.path().join("r5.ts"), "go"),
        Some(1),
        "inside the cap"
    );
    assert_eq!(
        locate(&chain.path().join("r0.ts"), "go"),
        None,
        "past the cap"
    );
}

/// Rendering refuses on its own grounds, after `locate` has said yes or while it says no: a
/// wrong argument count, a re-export cycle, a chain past the hop cap, and a helper file the
/// semantic builder rejects. The first row is the control that rendering works at all.
#[test]
fn rendering_a_helper_refuses_on_arity_cycles_hop_cap_and_semantic_errors() {
    let dir = helper_dir(&[
        ("run.ts", "export function run(a) { return a; }"),
        (
            "bad.ts",
            "export function run(a) { let b = 1; let b = 2; return a; }",
        ),
    ]);
    let cycle = cycle_dir();
    let render = |path: std::path::PathBuf, export: &str, arguments: &[&str]| {
        let arguments: Vec<String> = arguments.iter().map(|a| (*a).to_owned()).collect();
        render_inline(&origin_at(path, export), &arguments, Mode::Alpha, 0)
    };
    assert!(
        render(dir.path().join("run.ts"), "run", &["x"]).is_some(),
        "the control renders"
    );
    assert_eq!(
        render(dir.path().join("run.ts"), "run", &["x", "y"]),
        None,
        "an arity mismatch refuses"
    );
    assert_eq!(
        render(cycle.path().join("a.ts"), "spin", &[]),
        None,
        "a re-export cycle refuses"
    );
    assert_eq!(
        render(dir.path().join("bad.ts"), "run", &["x"]),
        None,
        "a semantic error refuses"
    );

    let chain = reexport_chain(MAX_REEXPORT_HOPS + 2);
    let from_start = origin_at(chain.path().join("r0.ts"), "go");
    assert_eq!(
        render_inline(&from_start, &["x".to_owned()], Mode::Alpha, 0),
        None,
        "past the cap"
    );
    let near_end = origin_at(chain.path().join("r5.ts"), "go");
    assert!(
        render_inline(&near_end, &["x".to_owned()], Mode::Alpha, 0).is_some(),
        "inside the cap"
    );
}

// -- the C10 export-resolution contract ------------------------------------------------------

/// The contract names the declaring file and the name that file's own exports admit, through
/// direct exports, same-file aliases, and re-export chains that rename mid-chain. A terminal
/// is self-contained: resolving from it again reaches itself with no hops.
#[test]
fn export_resolution_names_the_declaring_file_and_its_own_export_name() {
    let dir = helper_dir(&[
        ("direct.ts", "export function run(a) { return a; }"),
        (
            "alias.ts",
            "function check(a) { return a; }\nexport { check as verify };",
        ),
        ("index.ts", "export { go } from './mid';"),
        ("mid.ts", "export { run as go } from './inner';"),
        ("inner.ts", "export function run(a, b) { return a; }"),
    ]);
    let terminal = |file: &str, export: &str| {
        let resolved = resolve_export(&dir.path().join(file), export)
            .unwrap_or_else(|refusal| panic!("{file}:{export} refused: {refusal:?}"));
        (resolved.path.canonicalize().unwrap(), resolved.export)
    };
    // Canonicalized on both sides: the walk keeps a start file's path verbatim while
    // resolver-produced paths get their own spelling per platform (macOS tempdirs sit
    // behind a /var -> /private/var symlink; Windows resolves to 8.3 short names without
    // the \\?\ prefix std's canonicalize adds), so only canonical forms compare equal.
    let canonical = |file: &str| dir.path().join(file).canonicalize().unwrap();
    assert_eq!(
        terminal("direct.ts", "run"),
        (canonical("direct.ts"), "run".to_owned()),
        "a direct export's terminal is the declaring file under its own name"
    );
    assert_eq!(
        terminal("alias.ts", "verify"),
        (canonical("alias.ts"), "verify".to_owned()),
        "a same-file alias's terminal stays in that file, keyed by the exported name"
    );
    assert_eq!(
        terminal("index.ts", "go"),
        (canonical("inner.ts"), "run".to_owned()),
        "a chain that renames mid-hop ends at the declaring file, under the name it \
         declares there"
    );

    // Idempotence: re-resolving a terminal reaches itself, so a consumer re-parsing the
    // terminal file and looking the export up again cannot drift from the walk.
    let resolved = resolve_export(&dir.path().join("index.ts"), "go").unwrap();
    assert_eq!(
        resolve_export(&resolved.path, &resolved.export)
            .as_ref()
            .err(),
        None,
        "a terminal resolves from itself with no hops"
    );
}

/// Every refusal the walk can end in is an explicit, distinct reason. The rows are one
/// edit away from their nearest positive in the tests above, so a reason firing on the
/// wrong shape is caught, not just a reason existing.
#[test]
fn export_refusals_carry_explicit_reasons() {
    // An export name the file simply does not have.
    let absent_name = helper_dir(&[("helper.ts", "export function other(a) { return a; }")]);
    assert_eq!(
        resolve_export(&absent_name.path().join("helper.ts"), "run"),
        Err(ExportRefusal::MissingExport),
        "an absent export name"
    );
    // A present export of an unadmitted shape folds into the same reason: `find_export`
    // cannot distinguish it and this contract adds no second opinion.
    let unadmitted = helper_dir(&[("helper.ts", "export const run = 5;")]);
    assert_eq!(
        resolve_export(&unadmitted.path().join("helper.ts"), "run"),
        Err(ExportRefusal::MissingExport),
        "a present export of an unadmitted shape is the missing-export refusal"
    );

    let dangling = helper_dir(&[("dangling.ts", "export { x } from './nowhere';")]);
    assert_eq!(
        resolve_export(&dangling.path().join("dangling.ts"), "x"),
        Err(ExportRefusal::UnresolvedSpecifier),
        "a re-export hop whose specifier resolves nowhere"
    );

    let absent_file = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_export(&absent_file.path().join("absent.ts"), "run"),
        Err(ExportRefusal::UnreadableOrUnparseable),
        "a file that cannot be read"
    );
    let broken = helper_dir(&[("broken.ts", "export function broken( {")]);
    assert_eq!(
        resolve_export(&broken.path().join("broken.ts"), "broken"),
        Err(ExportRefusal::UnreadableOrUnparseable),
        "a file that does not parse"
    );

    let cycle = cycle_dir();
    assert_eq!(
        resolve_export(&cycle.path().join("a.ts"), "spin"),
        Err(ExportRefusal::ReexportCycle),
        "an export chain that returns to a (path, export) it already resolved through"
    );

    // Installed packages are refused before they are read, both as the chain's start and
    // as a first-party barrel's terminal hop.
    let package = package_and_barrel();
    assert_eq!(
        resolve_export(
            &package
                .path()
                .join("node_modules")
                .join("a-pkg")
                .join("index.js"),
            "run"
        ),
        Err(ExportRefusal::InstalledPackage),
        "a chain starting inside an installed package"
    );
    assert_eq!(
        resolve_export(&package.path().join("barrel.ts"), "run"),
        Err(ExportRefusal::InstalledPackage),
        "a first-party barrel whose terminal hop lands in an installed package"
    );
}

/// The exact hop boundary, on both sides: a chain of exactly [`MAX_REEXPORT_HOPS`] hops
/// resolves to the declaring file, one hop more refuses with the hop-limit reason. This is
/// the pin that keeps this walk and the inline walks' loops from silently disagreeing about
/// what the shared bound means.
#[test]
fn the_reexport_hop_limit_is_exact() {
    // reexport_chain(N) is N hops: r0 -> ... -> rN defines `go`. From r1: exactly
    // MAX_REEXPORT_HOPS hops; from r0: one more.
    let chain = reexport_chain(MAX_REEXPORT_HOPS + 1);
    let resolved = resolve_export(&chain.path().join("r1.ts"), "go")
        .expect("a chain of exactly MAX_REEXPORT_HOPS hops resolves");
    // Canonicalized on both sides, like the terminal test above: a resolver-produced path
    // and a canonicalized one spell one file differently per platform (macOS /var symlink;
    // Windows 8.3 short names without the \\?\ prefix), so only canonical forms compare.
    assert_eq!(
        resolved.path.canonicalize().unwrap(),
        chain
            .path()
            .join(format!("r{}.ts", MAX_REEXPORT_HOPS + 1))
            .canonicalize()
            .unwrap(),
        "the terminal is the chain's declaring file"
    );
    assert_eq!(
        resolve_export(&chain.path().join("r0.ts"), "go"),
        Err(ExportRefusal::HopLimit),
        "one hop past the bound refuses with the hop-limit reason"
    );
}

/// The visited identity is canonical, so a cycle whose start was spelled differently from
/// the resolver's spelling of the same file (`./`-component, symlinked or short-named
/// directory) is still named a cycle. The construction lands the start's first revisit
/// exactly on the hop boundary: eight files (`a` plus seven intermediates) means the return
/// to `a` is iteration 8, inside the loop, while the lap the miss would have cost never
/// runs -- without the canonical identity this exact chain refuses with the hop-limit
/// reason instead.
#[test]
fn a_misspelled_start_still_names_a_cycle_at_the_hop_boundary() {
    let mut files = vec![("a.ts".to_owned(), "export { spin } from './c1';".to_owned())];
    for index in 1..MAX_REEXPORT_HOPS {
        let target = if index == MAX_REEXPORT_HOPS - 1 {
            "a".to_owned()
        } else {
            format!("c{}", index + 1)
        };
        files.push((
            format!("c{index}.ts"),
            format!("export {{ spin }} from './{target}';"),
        ));
    }
    let sources: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect();
    let dir = helper_dir(&sources);
    // `./` in the start spelling: canonicalized away on every platform, but never present
    // in a `resolve_hop` result, so the two spellings of `a.ts` differ as strings.
    assert_eq!(
        resolve_export(&dir.path().join("./a.ts"), "spin"),
        Err(ExportRefusal::ReexportCycle),
        "the start's first revisit is detected through the canonical identity"
    );
}
