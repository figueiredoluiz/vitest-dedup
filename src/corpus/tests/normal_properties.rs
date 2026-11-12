//! Reproducible normal-form and identity properties over owned corpus callbacks.

use super::{corpora, owned_findings};
use crate::framework::{vitest::DIALECT, Registers};
use crate::inventory::{Inventory, SiteKind};
use crate::lang::{NodeId, Parsed};
use crate::model::Rule;
use crate::normal::{Exports, Normalizer, Setting, Structure};
use proptest::prelude::*;
use std::collections::HashMap;

const OBJECTS: &str =
    include_str!("../../../fixtures/duplicates/object-property-equivalents.test.ts");
const PRIVATE: &str =
    include_str!("../../../fixtures/duplicates/nested-private-owner-renamed.test.ts");
const LITERALS: &str =
    include_str!("../../../fixtures/duplicates/not-duplicates-literal-variation.test.ts");
const ROWS: &str = include_str!("../../../fixtures/duplicates/each-row-duplicate-rows.test.ts");
const JSX: &str =
    include_str!("../../../fixtures/duplicates/duplicates-jsx-formatting-only.test.tsx");
const HEADER: &str = "import { test, expect } from 'vitest';\n";
const TEMPLATES: &str =
    include_str!("../../../fixtures/duplicates/duplicates-name-observed-coercion.test.ts");

/// Every recorded test callback, requiring a complete parse and the expected population first.
fn roots(
    parsed: &Parsed,
    found: &Inventory,
    normalizer: &Normalizer,
    expected: usize,
) -> Vec<NodeId> {
    assert_eq!(parsed.parse_errors, 0);
    let sites: Vec<_> = found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Test))
        .collect();
    assert_eq!(sites.len(), expected);
    sites
        .into_iter()
        .map(|site| normalizer.callback(site.node).unwrap().function.unwrap())
        .collect()
}

/// The written callbacks of an owned source, preserving its executable body text.
fn callbacks(source: &str, expected: usize) -> Vec<String> {
    crate::testing::project(source, &[], false, |_, parsed, found, normalizer| {
        roots(parsed, found, normalizer, expected)
            .into_iter()
            .map(|root| {
                let span = parsed.tree.span(root);
                source[span.start as usize..span.end as usize].to_owned()
            })
            .collect()
    })
}

/// Replace one known corpus anchor, refusing stale or ambiguous transformations.
fn replace(source: &str, from: &str, to: &str) -> String {
    assert_eq!(source.matches(from).count(), 1, "{from:?} in {source}");
    source.replacen(from, to, 1)
}

/// Rename each resolved local and parameter without changing property names or other bindings.
fn renamed(callback: &str, nonce: u16) -> String {
    let source = format!("{HEADER}test('seed', {callback});");
    crate::testing::project(&source, &[], false, |_, parsed, found, normalizer| {
        let root = parsed.tree.span(roots(parsed, found, normalizer, 1)[0]);
        let mut edits = Vec::new();
        for (index, id) in parsed.scopes.symbols().enumerate() {
            let symbol = parsed.scopes.symbol(id);
            if !root.contains(symbol.span) {
                continue;
            }
            let name = format!("xc27_{nonce}_{index}");
            assert!(!source.contains(&name));
            for span in std::iter::once(symbol.span)
                .chain(symbol.references.iter().map(|reference| reference.span))
            {
                assert!(root.contains(span));
                assert_eq!(&source[span.start as usize..span.end as usize], symbol.name);
                edits.push((span.start - root.start, span.end - root.start, name.clone()));
            }
        }
        assert!(!edits.is_empty());
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
        edits.dedup();
        let mut output = callback.to_owned();
        for (start, end, name) in edits {
            output.replace_range(start as usize..end as usize, &name);
        }
        output
    })
}

/// Count eligible cases before any identity or finding assertion.
fn eligible(source: &str, expected: usize) -> super::FileCorpus {
    let corpus = corpora(&[("a.test.tsx", source)], &["a.test.tsx"]).remove(0);
    assert_eq!(corpus.tests.len(), expected, "{source}");
    assert!(
        corpus.tests.iter().all(|case| case.exclusion.is_none()),
        "{corpus:#?}"
    );
    corpus
}

/// Require the positive pair to survive the final rule evaluation.
fn duplicate_pair(corpus: super::FileCorpus) {
    let findings: Vec<_> = owned_findings("a.test.tsx", corpus)
        .findings
        .into_iter()
        .filter(|finding| finding.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].titles, ["original", "copy"]);
}

/// Flat and structural forms repeat after sibling queries warm the module's caches.
#[test]
fn xc27_normal_forms_repeat_in_any_query_order() {
    let seeds = prop::sample::select(vec![(OBJECTS, 14), (PRIVATE, 4)]);
    crate::testing::property(
        64,
        (seeds, 0_usize..18, any::<bool>()),
        |((source, count), offset, reverse)| {
            crate::testing::project(source, &[], false, |path, parsed, found, normalizer| {
                let roots = roots(parsed, found, normalizer, count);
                let expected: Vec<_> = roots
                    .iter()
                    .map(|&root| {
                        (
                            normalizer.of(root),
                            normalizer.shape(root, HashMap::new(), false),
                        )
                    })
                    .collect();
                let structure = Structure::of(parsed, found);
                let exports = Exports::default();
                let file = path.join("a.test.tsx");
                let mut fresh = Normalizer::new(
                    &file,
                    parsed,
                    Setting {
                        dialect: &DIALECT,
                        root: path,
                        exports: &exports,
                    },
                    &structure,
                );
                let mut order: Vec<_> = (0..roots.len()).collect();
                let length = order.len();
                order.rotate_left(offset % length);
                if reverse {
                    order.reverse();
                }
                for index in order {
                    let root = roots[index];
                    let _ = normalizer.parameterization_uncertain(root);
                    prop_assert_eq!(normalizer.of(root), expected[index].0.clone());
                    prop_assert_eq!(
                        normalizer.shape(root, HashMap::new(), false),
                        expected[index].1.clone()
                    );
                    prop_assert_eq!(fresh.of(root), expected[index].0.clone());
                    prop_assert_eq!(
                        fresh.shape(root, HashMap::new(), false),
                        expected[index].1.clone()
                    );
                    prop_assert!(!normalizer.take_truncated() && !fresh.take_truncated());
                }
                Ok(())
            })
        },
    );
}

/// Local object and parameter names stay alpha-equivalent beside an independent name observation.
#[test]
fn xc27_local_renaming_retains_duplicates() {
    let bodies = callbacks(OBJECTS, 14);
    let seeds = prop::sample::select(vec![
        bodies[0].clone(),
        bodies[2].clone(),
        bodies[4].clone(),
        bodies[6].clone(),
    ]);
    crate::testing::property(
        128,
        (seeds, any::<u16>(), any::<bool>()),
        |(mut callback, nonce, observes_name)| {
            if observes_name {
                let body = callback.strip_suffix('}').unwrap();
                callback = format!("{body}; expect((() => 0).name).toBe(''); }}");
            }
            let other = renamed(&callback, nonce);
            prop_assert_ne!(&callback, &other);
            let source = format!("{HEADER}test('original', {callback}); test('copy', {other});");
            let corpus = eligible(&source, 2);
            prop_assert_eq!(corpus.tests[0].key, corpus.tests[1].key);
            duplicate_pair(corpus);
            Ok(())
        },
    );
}

/// Exact identity preserves consumed literals and argument order while retaining an identical pair.
#[test]
fn xc27_literals_and_argument_order_keep_distinct_identities() {
    let callback = callbacks(LITERALS, 2).remove(0);
    crate::testing::property(128, (1_i32..50, 1_i32..50), |(left, difference)| {
        let right = left + difference;
        let original = replace(&callback, "area(2, 3)", &format!("area({left}, {right})"));
        let original = replace(&original, "toBe(6)", &format!("toBe({})", left * right));
        let literal = replace(
            &original,
            &format!("area({left}, {right})"),
            &format!("area({}, {right})", left + 1),
        );
        let order = replace(
            &original,
            &format!("area({left}, {right})"),
            &format!("area({right}, {left})"),
        );
        let source = format!(
            "{HEADER}const area = (a, b) => a * b;
             test('original', {original}); test('copy', {original});
             test('literal', {literal}); test('order', {order});"
        );
        let corpus = eligible(&source, 4);
        prop_assert_eq!(corpus.tests[0].key, corpus.tests[1].key);
        prop_assert_ne!(corpus.tests[0].key, corpus.tests[2].key);
        prop_assert_ne!(corpus.tests[0].key, corpus.tests[3].key);
        prop_assert_ne!(corpus.tests[2].key, corpus.tests[3].key);
        duplicate_pair(corpus);
        Ok(())
    });
}

/// Only repeated consumed row inputs share a row class and a duplicate-row finding.
#[test]
fn xc27_consumed_row_inputs_keep_their_identity() {
    let callback = callbacks(ROWS, 1).remove(0);
    crate::testing::property(128, (1_i32..50, 1_i32..50), |(left, difference)| {
        let right = left + difference;
        let source = format!(
            "{HEADER}test.each([[{left}], [{left}], [{right}]])('doubles %i', {callback});"
        );
        let corpus = eligible(&source, 3);
        prop_assert!(corpus.tests.iter().all(|case| case.row_class.is_some()));
        prop_assert_eq!(corpus.tests[0].row_class, corpus.tests[1].row_class);
        prop_assert_ne!(corpus.tests[0].row_class, corpus.tests[2].row_class);
        let findings: Vec<_> = owned_findings("a.test.tsx", corpus)
            .findings
            .into_iter()
            .filter(|finding| finding.rule == Rule::DuplicateEachRow)
            .collect();
        prop_assert_eq!(findings.len(), 1);
        prop_assert_eq!(&findings[0].titles, &vec![format!("doubles {left}"); 2]);
        Ok(())
    });
}

/// Layout whitespace folds while observable first- and last-line spaces retain their identity.
#[test]
fn xc27_jsx_layout_preserves_observable_edge_spaces() {
    let callback = callbacks(JSX, 2).remove(0);
    crate::testing::property(128, ("[a-z]{1,8}", 1_usize..5), |(word, padding)| {
        let indent = " ".repeat(padding);
        let text = [
            word.clone(),
            format!("\n{indent}{word}\n{indent}"),
            format!(" {word}"),
            format!("{word} "),
        ];
        let mut source = HEADER.to_owned();
        for (index, text) in text.iter().enumerate() {
            let body = replace(&callback, ">Save</Button>", &format!(">{text}</Button>"));
            source.push_str(&format!("test('case {index}', {body});"));
        }
        crate::testing::project(&source, &[], false, |_, parsed, found, normalizer| {
            let forms: Vec<_> = roots(parsed, found, normalizer, 4)
                .into_iter()
                .map(|root| normalizer.of(root))
                .collect();
            prop_assert_eq!(&forms[0], &forms[1]);
            prop_assert_ne!(&forms[0], &forms[2]);
            prop_assert_ne!(&forms[0], &forms[3]);
            Ok(())
        })
    });
}

/// Untagged interpolations read cooked text, preserving equal spellings and conflicting values.
#[test]
fn xc27_template_spellings_keep_cooked_text_identity() {
    let callback = callbacks(TEMPLATES, 8).remove(2);
    crate::testing::property(128, b'a'..=b'z', |letter| {
        let character = char::from(letter);
        let different = char::from(b'a' + (letter - b'a' + 1) % 26);
        let original = replace(&callback, "`${alpha}`", &format!("`{character}${{alpha}}`"));
        let escaped = replace(
            &callback,
            "`${alpha}`",
            &format!("`\\x{letter:02x}${{alpha}}`"),
        );
        let conflict = replace(&callback, "`${alpha}`", &format!("`{different}${{alpha}}`"));
        let source = format!(
            "{HEADER}test('original', {original}); test('copy', {escaped}); test('conflict', {conflict});"
        );
        let corpus = eligible(&source, 3);
        prop_assert_eq!(corpus.tests[0].key, corpus.tests[1].key);
        prop_assert_ne!(corpus.tests[0].key, corpus.tests[2].key);
        duplicate_pair(corpus);
        Ok(())
    });
}
