//! Immediate calls through a fresh object's selected own data field.

use super::*;

/// Count task wrappers while preserving an independently registered direct control.
fn task_wrappers(expression: &str) -> usize {
    let sites = fixture_call_sites(
        "import {test as api} from 'vitest'; import * as v from 'vitest';",
        expression,
    );
    sites
        .into_iter()
        .filter(|site| site.kind == SiteKind::Wrapper(Reach::Tasks))
        .count()
}

/// A static own field selects the same fixture builder regardless of field order and syntax.
#[test]
fn own_object_fields_account_for_the_selected_call() {
    for key in ["run", "'run'", "['run']"] {
        for access in [".run", "['run']"] {
            for value in ["api.extend({})", "(v.test['extend']({}) as any)"] {
                for properties in [
                    format!("{key}: {value}, unrelated: 0"),
                    format!("unrelated: 0, {key}: {value}"),
                ] {
                    let source = format!("({{{properties}}} as any){access}('chosen', () => {{}})");
                    assert_eq!(task_wrappers(&source), 1, "{source}");
                }
            }
        }
    }
}

/// A computed prototype key defines a field, even beside a prototype-setting initializer.
#[test]
fn computed_prototype_fields_are_own_data_fields() {
    for properties in [
        "['__proto__']: api.extend({})",
        "__proto__: null, ['__proto__']: api.extend({})",
        "['__proto__']: api.extend({}), '__proto__': null",
    ] {
        let source = format!("({{{properties}}}).__proto__('chosen', () => {{}})");
        assert_eq!(task_wrappers(&source), 1, "{source}");
    }
}

/// A TypeScript assertion leaves both unique and duplicate computed string keys unchanged.
#[test]
fn asserted_computed_keys_keep_their_string_identity() {
    for key in ["'run' as const", "('run' satisfies string)", "('run')!"] {
        let unique = format!("({{[{key}]: api.extend({{}})}}).run('chosen', () => {{}})");
        let duplicate =
            format!("({{run: api.extend({{}}), [{key}]: () => {{}}}}).run('other', () => {{}})");
        assert_eq!(task_wrappers(&unique), 1, "{unique}");
        assert_eq!(task_wrappers(&duplicate), 0, "{duplicate}");
    }
}

/// Dynamic, competing, inherited and indirect selections gain no own-field proof.
#[test]
fn uncertain_object_fields_do_not_account_for_a_new_call() {
    for source in [
        "({run: api.extend({}), ['run']: () => {}}).run('changed', () => {})",
        "({...candidate, run: api.extend({})}).run('spread before', () => {})",
        "({run: api.extend({}), ...candidate}).run('spread after', () => {})",
        "({run: api.extend({}), [key]: () => {}}).run('unknown key', () => {})",
        "({run: api.extend({})})[key]('unknown selection', () => {})",
        "({get run() { return api.extend({}) }}).run('getter', () => {})",
        "({run() { return api.extend({}) }}).run('method', () => {})",
        "({__proto__: api.extend({})}).__proto__('prototype', () => {})",
        "({other: api.extend({})}).run('missing field', () => {})",
        "({run: (api.extend({}), () => {})}).run('inert value', () => {})",
        "const box = {run: api.extend({})}; box.run('aliased object', () => {})",
    ] {
        assert_eq!(task_wrappers(source), 0, "{source}");
    }
}

/// Earlier initializers can replace the extension. Later writes leave the captured value alone.
#[test]
fn own_field_reads_the_extension_at_its_initializer() {
    for write in [
        "api.extend = () => () => {}",
        "api['extend'] = () => () => {}",
        "Object.defineProperty(api, 'extend', {value: () => () => {}})",
    ] {
        for (properties, expected) in [
            (format!("before: ({write}), run: api.extend({{}})"), 0),
            (format!("run: api.extend({{}}), after: ({write})"), 1),
        ] {
            let source = format!("({{{properties}}}).run('selected', () => {{}})");
            assert_eq!(task_wrappers(&source), expected, "{source}");
        }
    }
}

/// Earlier registrations preserve selection only with the shared direct-call and callback proof.
#[test]
fn own_field_prefix_uses_the_existing_builder_proof() {
    for (prefix, expected) in [
        ("api('prefix', () => {})", 1),
        ("api('prefix', function body() {})", 1),
        ("api(title(), () => {})", 0),
        ("api('prefix', namedCallback)", 0),
        ("v.test('prefix', () => {})", 0),
        ("unknown()", 0),
    ] {
        let source =
            format!("({{prefix: {prefix}, run: api.extend({{}})}}).run('selected', () => {{}})");
        assert_eq!(task_wrappers(&source), expected, "{source}");
    }
}
