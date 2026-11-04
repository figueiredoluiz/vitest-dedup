//! Which import specifiers name an installable package.

/// `module.builtinModules` of Node 26 without its prefixed entries, plus the `_stream_*` names that
/// earlier versions still list. Matched exactly, subpaths included, so `fs/promises` is a builtin
/// and `fs/not-real` is a package lookup.
const NODE_BUILTINS: &[&str] = &[
    "_http_agent",
    "_http_client",
    "_http_common",
    "_http_incoming",
    "_http_outgoing",
    "_http_server",
    "_tls_common",
    "_stream_duplex",
    "_stream_passthrough",
    "_stream_readable",
    "_stream_transform",
    "_stream_wrap",
    "_stream_writable",
    "_tls_wrap",
    "assert",
    "assert/strict",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "dns/promises",
    "domain",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "inspector",
    "inspector/promises",
    "module",
    "net",
    "os",
    "path",
    "path/posix",
    "path/win32",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "readline/promises",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "util/types",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// Whether `specifier` is spelled like a request for an installed package rather than as something
/// that is plainly not one: a relative, absolute or `#`-internal path, a `~`, `~~`, `@@` or `@/` alias, a URI
/// scheme or drive path (a colon in the first segment, `npm:` and `jsr:` included), a backslash
/// path, a package name with whitespace, or a scope with no name. Only the package part is judged: a
/// subpath or a `?query` suffix may contain anything, and legacy names are accepted, because
/// refusing a real package would hide a missing install. An alias written like a package
/// (`@app/missing`) cannot be told apart by its spelling.
fn has_package_syntax(specifier: &str) -> bool {
    let head = specifier.split(['?', '#']).next().unwrap_or_default();
    if head.is_empty()
        || head.starts_with(['.', '/'])
        || ["~", "~~", "@@"].contains(&head)
        || ["~/", "~~/", "@@/"]
            .iter()
            .any(|alias| head.starts_with(alias))
        || head.contains('\\')
    {
        return false;
    }
    let mut parts = head.split('/');
    let first = parts.next().unwrap_or_default();
    let plain = |segment: &str| !segment.contains(':') && !segment.contains(char::is_whitespace);
    match first.strip_prefix('@') {
        Some(scope) => {
            let name = parts.next().unwrap_or_default();
            !scope.is_empty() && !name.is_empty() && plain(scope) && plain(name)
        }
        None => plain(first),
    }
}

/// Whether `specifier` names an installable package the analyzer does not resolve by name.
///
/// Node's prefix-only modules (`node:sqlite`, `node:sea`, `node:test`) are not in the builtin list:
/// without the prefix those names are ordinary packages, and `test` is a real one.
pub(crate) fn is_package_specifier(specifier: &str) -> bool {
    has_package_syntax(specifier)
        && !(specifier.starts_with("node:")
            || NODE_BUILTINS.contains(&specifier)
            || framework_package(specifier))
}

/// Whether `specifier` is the framework's own module, a subpath of it, or a package of its scope:
/// the analyzer reads the framework by name, never through an install.
fn framework_package(specifier: &str) -> bool {
    crate::framework::current()
        .dialect()
        .api_modules
        .iter()
        .any(|module| {
            specifier == *module
                || specifier
                    .strip_prefix(module)
                    .is_some_and(|rest| rest.starts_with('/'))
                || specifier
                    .strip_prefix('@')
                    .and_then(|rest| rest.strip_prefix(module))
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

#[cfg(test)]
mod tests {
    use super::is_package_specifier;

    /// The framework, its subpaths and its scope are read by name; a lookalike and Node builtins
    /// are classified on their own terms.
    #[test]
    fn the_framework_and_its_scope_are_not_installable_packages() {
        let framework = crate::framework::current().dialect().api_modules[0];
        for name in [
            framework.to_owned(),
            format!("{framework}/config"),
            format!("@{framework}/spy"),
        ] {
            assert!(!is_package_specifier(&name), "{name}");
        }
        for name in [
            format!("{framework}-mock-extended"),
            format!("@{framework}-x/tool"),
            "lodash/fp".to_owned(),
            "left-pad".to_owned(),
        ] {
            assert!(is_package_specifier(&name), "{name}");
        }
        for name in ["fs", "node:fs", "node:sqlite", "./local", "/abs"] {
            assert!(!is_package_specifier(name), "{name}");
        }
    }
}
