//! Versioned owned-core evidence. Legacy fingerprint fields are deliberately not synthesized.
use super::{bounded_text, report_safe_path, HOOK_CONTENT_NOTICE, REGISTRATION_TEST_NOTICE};
use crate::corpus::{
    records::{DiagnosticHook, Views},
    Exclusion, TestCase,
};
use crate::normal::digest;
use anyhow::Result;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Hashes are opaque, run-local equality evidence, never a raw form or baseline identity.
fn key(value: u64) -> String {
    format!("{value:016x}")
}

/// Independent normalization dimensions; unavailable shape/trace evidence stays null.
fn views(value: Option<&Views>, test: bool) -> Value {
    match value {
        None => Value::Null,
        Some(v) => {
            let mut result =
                json!({"body":key(v.body), "shape":v.shape.map(key), "trace":v.trace.map(key)});
            if test {
                result["context"] = json!(key(v.context));
                result["modifiers"] = json!(key(v.modifiers));
            }
            result
        }
    }
}

/// Sanitize and bound every repository-controlled identity before serializing a record.
fn header(kind: &str, path: &Path, line: u32, column: u32) -> (Value, Vec<String>) {
    let mut truncated = Vec::new();
    let path = bounded_text(&report_safe_path(path), "path", &mut truncated);
    (
        json!({"type":kind,"schemaVersion":"0.2","engine":"owned","path":path,"line":line,"column":column}),
        truncated,
    )
}

/// Render one execution, keeping declaration identity distinct from caller ownership.
fn test(path: &Path, case: &TestCase) -> Value {
    let (mut record, mut truncated) = header(
        "test",
        case.declaration_path(path),
        case.span.start.line,
        case.span.start.column,
    );
    let title = case
        .name
        .as_ref()
        .map(|name| name.join(" > "))
        .or_else(|| case.full_title.clone())
        .or_else(|| case.title.clone())
        .unwrap_or_default();
    record["title"] = json!(bounded_text(&title, "title", &mut truncated));
    if case.declaration_path(path) != path {
        record["registrationFile"] = json!(bounded_text(
            &report_safe_path(path),
            "registrationFile",
            &mut truncated
        ));
    }
    let compared = case.exclusion.is_none();
    record["row"] = json!(case.row);
    record["comparable"] = json!(compared);
    record["exclusion"] = json!(case.exclusion.map(|exclusion| match exclusion {
        Exclusion::EmptyBody => "empty-body",
        Exclusion::NoCallback => "no-callback",
        other => other
            .reason()
            .expect("nontrivial exclusion has a reason")
            .0
            .code(),
    }));
    record["traceComplete"] = json!(case.trace_complete);
    record["ordinaryEvents"] = json!(case.ordinary_events);
    let near_eligible = compared
        && case.trace_complete
        && case.ordinary_events >= crate::analysis::MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE;
    let subsumption_eligible = compared && case.trace_complete && case.ordinary_events != 0;
    let mut events = case.trace.clone();
    events.sort_unstable();
    record["comparisonKeys"] = json!({
        "duplicate":compared.then(|| key(case.key)),
        "crossFile":compared.then(|| key(case.module_key)),
        "parameterization":compared.then_some(case.parameterization_key).flatten().map(key),
        "similarity":near_eligible.then(|| key(case.similarity_key)),
        "subsumption":subsumption_eligible.then_some(case.subsumption_key).flatten().map(key),
        "name":case.name.as_ref().map(|name| key(digest(&format!("{name:?}")))),
        "rowClass":case.row_class.map(key),
        "eventSet":near_eligible.then(|| key(digest(&format!("{events:?}"))))
    });
    record["views"] = views(case.views.as_deref(), true);
    record["truncatedFields"] = json!(truncated);
    record["contentNotice"] = json!(REGISTRATION_TEST_NOTICE);
    record
}

/// Render a declaration even when the hook rule has no chain or effect proof for it.
fn hook(path: &Path, hook: &DiagnosticHook) -> Value {
    let (mut record, truncated) =
        header("hook", path, hook.span.start.line, hook.span.start.column);
    record["kind"] = json!(hook.kind);
    record["comparisonKeys"] = json!({"effects":hook.effect_key.map(key)});
    record["views"] = views(hook.views.as_deref(), false);
    record["truncatedFields"] = json!(truncated);
    record["contentNotice"] = json!(HOOK_CONTENT_NOTICE);
    record
}

/// Stable hooks-first emission without copying the owned case/trace populations.
pub(super) fn write(
    files: &[(PathBuf, Vec<TestCase>)],
    hooks: &[(PathBuf, DiagnosticHook)],
    writer: &mut dyn Write,
) -> Result<()> {
    let mut hooks: Vec<_> = hooks.iter().collect();
    hooks.sort_by_key(|(path, hook)| {
        (
            report_safe_path(path),
            hook.span.start.line,
            hook.span.start.column,
        )
    });
    for (path, value) in hooks {
        serde_json::to_writer(&mut *writer, &hook(path, value))?;
        writer.write_all(b"\n")?;
    }
    let mut cases: Vec<_> = files
        .iter()
        .flat_map(|(path, cases)| cases.iter().map(move |case| (path, case)))
        .collect();
    cases.sort_by_key(|(path, case)| {
        (
            report_safe_path(case.declaration_path(path)),
            case.span.start.line,
            case.span.start.column,
            report_safe_path(path),
            case.row,
        )
    });
    for (path, case) in cases {
        serde_json::to_writer(&mut *writer, &test(path, case))?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}
