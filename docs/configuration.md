# Configuration

A `.vitest-dedup.json` file is found by walking upward from the analyzed directory, or named with
`--config`. Command-line flags override it.

```json
{
  "include": ["src/**/*.test.ts"],
  "exclude": ["**/fixtures/**"],
  "threshold": 0,
  "rules": { "subsumed-test": "warning" }
}
```

Every key is optional, and an unknown key is an error. `include` and `exclude` are globs relative
to the analyzed directory; `threshold` is a percentage from 0 to 100; `rules` maps a rule id to
`off`, `warning`, or `error`. Files matched by `.gitignore` or by a `.vitest-dedupignore` file in
the same syntax are skipped as well. Single `*` and `?` wildcards stay within a path
component; use `**` to match across directories. This also applies to resolved Vitest project
globs and project paths, and to the base revision `--baseline-from-ref` reads.

VitestDedup reads `test.include`, `test.exclude`, `test.globals`, project lists, and the typecheck
channel from `vitest.config.*` and `vite.config.*` without executing them.

Test file naming. `*.test.*` and `*.spec.*` are recognized by default. A Vitest config that
declares its own `include` replaces that list, so a project naming `**/*.unit.ts` gets exactly
those files, the same set Vitest runs. An `include` written in `.vitest-dedup.json` is an
instruction and is never widened by a project's broader list.

Monorepos resolve per project. A config declaring `test.projects`, or a `vitest.workspace.*` file,
is resolved entry by entry, honouring `extends: false`.

`globals: true` is detected from the resolved config. When the project list is built in a way no
static reader can follow, such as a bare variable or `mergeConfig` with a dynamic import, the
setting is reported as undetermined and the flag is needed:

```sh
vitest-dedup . --globals
```

When no executable tests are analyzed and no quality-gate failure applies, the command exits with
operational-error code 2 instead of treating an empty report as clean. A real suite-level quality
failure or too many new findings under `--fail-on-new` remains exit 1. If test-shaped files were
read but yielded no tests, the diagnostic reports that observation and suggests checking both
`test.globals` (or `--globals`) and build-time transforms such as auto-imports; it does not claim
either cause was proved. Known cases, such as statically empty `.each`/`.for` tables, receive a more
specific diagnostic. The flag is opt-in because ambient `describe` and `it` cannot be told apart
from Cypress, Mocha, or Jasmine without project configuration, and guessing would invent findings
in those files.

## Source resource limits

Source files are limited to 8 MiB. Before parsing, the frontend also refuses lexical bracket
nesting deeper than 256 and more than 8,192 raw syntax units. The latter counts opening brackets,
operator/member/label/escape punctuation and recursive control/type keywords, including occurrences in
strings, comments and regexes. This conservative bound prevents lexical ambiguity from hiding
stack-exhausting input; a wide generated file or punctuation-heavy literal can therefore exceed
it even when its actual syntax is shallow. Nonnegative integer payloads and commas do not consume
units. These are analyzer resource limits, not JavaScript or Vitest restrictions.

Refused files make analysis incomplete and produce diagnostics; findings in other files remain
available. First-party imported helpers use the same preflight limits. Use
`--fail-on-incomplete=reads` (or `--fail-on-incomplete`) to make refused input an operational
failure. Admitted extraction runs on an explicitly sized worker stack; a larger stack alone
would not protect against unbounded input.
