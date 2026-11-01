//! Whole-suite identities, kept outside the executable-test population.

use super::{Setting, TestCase};
use crate::framework::{Chain, Registers};
use crate::inventory::{Inventory, Label, SiteKind};
use crate::lang::{Lines, NodeId, Parsed};
use crate::model::{Position, Span};
use crate::normal::{self, digest, Normalizer, Structure};
use std::collections::HashMap;

/// Keys whole inline suites only when their names, ancestry and contained tests are comparable.
pub(super) fn collect(
    parsed: &Parsed,
    found: &Inventory,
    structure: &Structure,
    contexts: &HashMap<Option<NodeId>, String>,
    tests: &[TestCase],
    normalizer: &mut Normalizer<'_>,
    setting: (Setting<'_>, &str),
) -> Vec<TestCase> {
    let (setting, source) = setting;
    let tree = &parsed.tree;
    let sites: HashMap<_, _> = found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Suite))
        .map(|site| (site.node, site))
        .collect();
    let lines = Lines::new(source);
    let mut suites = Vec::new();
    for site in sites.values() {
        let Some(callback) = normalizer.callback(site.node).filter(|body| body.inline) else {
            continue;
        };
        let Some(body) = callback.function else {
            continue;
        };
        let Some(around) = normalizer.suites_around(site.node) else {
            continue;
        };
        let calls: Vec<_> = around
            .iter()
            .filter_map(|scope| structure.suites.get(scope).copied())
            .chain(std::iter::once(site.node))
            .collect();
        let mut names = Vec::new();
        let mut context = vec![contexts.get(&None).cloned().unwrap_or_default()];
        let mut eligible = true;
        for &call in &calls {
            let Some(owner) = sites.get(&call) else {
                eligible = false;
                break;
            };
            let Some(title) = normal::titles::title(parsed, call) else {
                eligible = false;
                break;
            };
            if owner
                .labels
                .iter()
                .any(|label| !matches!(label, Label::RegistrationBody))
                || owner.chain.iter().any(|member| {
                    matches!(
                        setting.dialect.chain_member(member),
                        Some(Chain::Skip | Chain::Todo | Chain::Table(_))
                    )
                })
                || !normal::row_identity::suite_prefix_known(parsed, call, setting.dialect)
            {
                eligible = false;
                break;
            }
            let Some(callee) = tree.child(call, "callee") else {
                eligible = false;
                break;
            };
            let Some(owner_body) = normalizer.callback(call) else {
                eligible = false;
                break;
            };
            let arguments: Vec<_> = tree.list(call, "arguments").into_iter().flatten().collect();
            if !normalizer.active_suite(callee, &arguments) {
                eligible = false;
                break;
            }
            names.push(title);
            if call != site.node {
                context.push(normalizer.of(callee));
                context.extend(
                    arguments
                        .into_iter()
                        .skip(1)
                        .filter(|&arg| arg != owner_body.argument)
                        .map(|arg| normalizer.of(arg)),
                );
            }
        }
        let children: Vec<_> = tests
            .iter()
            .filter(|test| {
                site.span.start <= test.span.start_byte && test.span.end_byte <= site.span.end
            })
            .collect();
        if !eligible
            || children.is_empty()
            || children.iter().any(|test| test.exclusion.is_some())
            || normalizer.order_dependent(body)
        {
            continue;
        }
        for scope in around {
            context.push(contexts.get(&Some(scope)).cloned().unwrap_or_default());
        }
        context.push(normalizer.of(site.node));
        let key = digest(&serde_json::to_string(&context).expect("strings serialize"));
        let position = |byte| {
            let (line, column) = lines.position(source, byte);
            Position { line, column }
        };
        suites.push(TestCase {
            span: Span {
                start: position(site.span.start),
                end: position(site.span.end),
                start_byte: site.span.start,
                end_byte: site.span.end,
            },
            title: names.last().cloned(),
            full_title: Some(names.join(" > ")),
            name: Some(names),
            key,
            module_key: 0,
            body: 0,
            parameterization_key: None,
            trace: Vec::new(),
            trace_complete: false,
            similarity_key: 0,
            subsumption_key: 0,
            callback_form: normal::CallbackForm::Interchangeable,
            row: None,
            row_class: None,
            inert: false,
            exclusion: None,
        });
    }
    suites.sort_by_key(|suite| suite.span.start_byte);
    suites
}
