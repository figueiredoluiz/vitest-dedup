# Configuration

A `.vitest-dedup.json` file is found by walking upward from the analyzed directory, or named with
`--config`. A relative `--config` path is resolved against the analyzed directory, not the shell's
working directory. Automatic discovery chooses the nearest file up to the filesystem root,
including files above a Git repository. Use `--config` to select a specific file instead.
Command-line flags override it.

```json
{
  "include": ["src/**/*.test.ts"],
  "exclude": ["**/fixtures/**"],
  "threshold": 0,
  "rules": { "subsumed-test": "warning" }
}
```

## Keys

Every key is optional, and an unknown key is an error.

| Key | Meaning |
| --- | --- |
| `include`, `exclude` | Globs relative to the analyzed directory. |
| `threshold` | A percentage from 0 to 100. |
| `rules` | Maps a rule id to `off`, `warning` or `error`. |

Files matched by `.gitignore` or by a `.vitest-dedupignore` file in the same syntax are skipped as
well. Ignore rules follow git. Inside a repository (the nearest directory at or above the analyzed
one holding `.git`, as a directory or as the file a worktree or submodule uses, or an existing `.jj`
entry), the `.gitignore` files from that repository's root down to the analyzed directory apply,
a deeper file overriding a shallower one, and so does the `.git/info/exclude` of a repository whose
`.git` is a directory. A `.gitignore` above the repository root does not, so a project nested in a
larger repository is not affected by that repository's rules. `.vitest-dedupignore` and `.ignore`
files are not git's and are read from every ancestor directory. Outside any repository every
`.gitignore` in the ancestor directories applies. A rule that names the analyzed directory itself
does not remove it.

## Glob semantics

Single `*` and `?` wildcards stay within a path component; use `**` to match across directories.
This also applies to resolved Vitest project globs and project paths, and to the base revision
`--baseline-from-ref` reads.

## How Vitest configuration is read

VitestDedup reads `test.include`, `test.exclude`, `test.globals`, project lists, and the typecheck
channel from `vitest.config.*` and `vite.config.*` without executing them.

### Test file naming

`*.test.*` and `*.spec.*` are recognized by default. A Vitest config that declares its own `include`
replaces that list, so a project naming `**/*.unit.ts` gets exactly those files. An `include`
written in `.vitest-dedup.json` is an instruction and is never widened by a project's broader list.

Differences from the set Vitest runs:

- The analyzer applies its own default `exclude` list when none is written: `node_modules`, `dist`,
  `build`, `coverage`, `.idea`, `.git`, `.cache`, `.output` and `.temp`. Vitest's default excludes
  only `node_modules` and `.git`, so test files under `build/` or `coverage/` are not analyzed.
- Hidden (dot-prefixed) directories are skipped.
- `includeSource` is additive. A file matching it is also scanned for in-source tests, whether or not
  `include` matches it.

### Monorepos

A config declaring `test.projects`, or a `vitest.workspace.*` file, is resolved entry by entry,
honouring `extends: false`.

Every path a configuration names must resolve inside the analyzed directory, after symlinks and
`..` segments are followed: project entries and globs, a project's string `extends`, a
`mergeConfig` base import, a project's `root` or `dir`, and tsconfig `paths`. A path that leaves it
is not read. The run reports which one and is incomplete: the entry is an unresolved project, or the
tsconfig import is unresolved. Paths that stay inside are not refused, including `..` that folds
back and symlinks that point within the tree. A re-export hop followed only to read an imported
constant in a `.each` table is the exception: its cell stays unresolved and nothing is reported,
neither the refusal nor a missing package or tsconfig on that chain. The root's own `vitest.config.*` or
`vitest.workspace.*` is held to the same rule, so a symlink to a file outside the directory is
refused. A refused `extends` or `mergeConfig` base is reported whatever the project sets for
itself, and whether or not any test runs under it. Relative imports in test files are not
configuration and may still reach outside. The import resolver may read a `package.json` under a
tsconfig-mapped directory outside the tree to decide which file inside it an import resolves to,
and follows a tsconfig `references` entry wherever it points.

A tsconfig `extends` target that does not exist (typically a generated file) or cannot be read, and a
missing `node_modules` when imports of listed dependencies fail, make the run incomplete and are
named with a fix; see [Missing prerequisites](incomplete-runs.md#missing-prerequisites).

### Globals

`globals: true` is detected from the resolved config. When the project list is built in a way no
static reader can follow, such as a bare variable or `mergeConfig` with a dynamic import, the
setting is reported as undetermined and the flag is needed:

```sh
vitest-dedup . --globals
```

The flag is opt-in because ambient `describe` and `it` cannot be told apart from Cypress, Mocha, or
Jasmine without project configuration, and guessing would invent findings in those files.

## Runs that analyze no tests

When no executable tests are analyzed and no quality-gate failure applies, the command exits with
operational-error code 2 instead of treating an empty report as clean. A real suite-level quality
failure or too many new findings under `--fail-on-new` remains exit 1.

- If test-shaped files were read but yielded no tests, the diagnostic reports that observation and
  suggests checking both `test.globals` (or `--globals`) and build-time transforms such as
  auto-imports. It does not claim either cause was proved.
- Known cases, such as statically empty `.each`/`.for` tables, receive a more specific diagnostic.

## Limits

Source size, nesting and work limits, and the memory budget, are on
[Incomplete runs and limits](incomplete-runs.md).

Back to the [documentation index](README.md).
