# Incomplete runs and limits

## What `incomplete` means

`incomplete` is true when some tests could not be compared: an unreadable config, a dynamic title, a
table that cannot be resolved statically, a limit reached. The absence of a finding is then not
evidence of its absence.

How each reporter says so:

| Reporter | Where |
| --- | --- |
| `terminal` | A banner before the findings. |
| `html` | A banner before the findings, plus `incompleteCauses` in the page's JSON island. |
| `json`, `jsonl` | The `incomplete` flag, plus `incompleteCauses`, which enumerates the reasons, counted. Excluded files are listed, up to 10,000; see [Excluded files](#excluded-files). |
| `sarif` | `runs[0].invocations[0].properties.incomplete` and `incompleteCauses`, keyed as in the `json` report. |

`--fail-on-incomplete` turns the flag into a non-zero exit. A file in which every test was excluded
is a blind spot; see [Blind spots](#blind-spots).

### Choosing which causes count

`--fail-on-incomplete` or `--fail-on-incomplete=any` counts every cause, because the classes call
for different responses.

- `=reads` counts only causes meaning the project could not be fully read: unreadable, oversized or
  partially read files, undetermined config, tests whose context could not be resolved, and
  [missing prerequisites](#missing-prerequisites).
- `=limits` counts only the analyzer's own bounded work being capped: the pair budget, the hook-scan
  budget, the memory budget, and truncated findings.

The value needs the `=`; with a space, `reads` would be read as the path.

### Missing prerequisites

Imports are resolved on disk. When a file or directory the project needs for that is absent, the run
is incomplete and says which one, instead of leaving imports unresolved without a word. Each entry
of `incompleteCauses.missingPrerequisites` (`json`, `jsonl`, `sarif`, `html`) has a `kind`, a `path`,
a `fix` and `unresolvedImports`, and the terminal and HTML banners print it as a sentence:

| `kind` | Reported when | `fix` |
| --- | --- | --- |
| `tsconfig-extends` | A tsconfig's `extends` target does not exist. Nothing under that tsconfig then resolves, relative imports included. | `nuxt prepare` for a path under `.nuxt` in a project with a `nuxt.config.*` or a `nuxt` dependency; the install command for a package target; otherwise `generate or restore this file`. |
| `tsconfig-unreadable` | The target exists but is not valid JSON. An empty file or one holding only comments loads as an empty config and is not reported; neither is a root tsconfig that cannot be read at all, which the resolver skips. | `generate or restore this file`. |
| `node-modules` | `package.json` lists dependencies, `node_modules` is missing or empty here and in every parent directory, and an import of a package could not be resolved. `vitest`, `node:` and Node core module imports do not count, nor do imports spelled as something other than a package (relative and absolute paths, `#` imports, `~/` and `@/` aliases, URLs, drive paths). An alias written like a package (`@app/missing`) cannot be told apart by its spelling and does count. | `pnpm install`, `bun install`, `yarn install` or `npm ci` for the nearest `pnpm-lock.yaml`, `bun.lock`/`bun.lockb`, `yarn.lock` or `package-lock.json`; otherwise `install the dependencies`. |

`unresolvedImports` counts the import resolutions that failed for that reason, once per analyzed
file and specifier. The `package.json` consulted is the nearest one at or above the analyzed directory. A project that never needed the file is not told about it: no `extends`, no
listed dependencies, or only relative imports produce nothing. Only resolution failures are
reported, so a tsconfig that is merely unusual is silent.

Limits: detection relies on what the resolver reports. A target that cannot be read because of an
I/O error (invalid UTF-8, no read permission) is swallowed by the resolver during discovery and is not
reported. On Windows a tsconfig is not found from a canonical importer path at all, so the two
`tsconfig-*` kinds are not detected there (the same gap that keeps `paths` aliases from applying); the
`node-modules` kind does not depend on tsconfig discovery. The failures are collected process-wide and
drained once per run, so the reports describe one analysis per process.

### Blind spots

A file in which every test was excluded produced no findings. It is named in the `json` and `jsonl` excluded-file list, but nowhere among the findings.
Blind spots are named on stderr, worst first, counted in the `blindFiles` field of the `json` and
`jsonl` summaries, and gated by `--fail-on-incomplete=reads`, since a wholly excluded file is the
extreme form of an unresolved-context cause.

### Excluded files

The terminal names at most ten wholly excluded files and twenty diagnostics. The `json` and `jsonl`
reports list the files that were excluded from comparison in whole or in part, up to 10,000 of
them, each with a reason code, so a run that excluded half a project can be triaged from the report
alone. Past the cap the files with the fewest excluded tests are left out of the list and counted in
`exclusions.omitted`; the summary totals always count every file.

- `json`: `excludedFiles`, sorted by path. `jsonl`: one `excluded-file` record per file, after the
  findings and before the summary; a run that excluded nothing writes none.
- Each entry has `path`, `reason`, `otherReasons`, `detail` (the cause; for a refused helper, the sentence the terminal prints),
  `testsExcluded`, `testsTotal` and `everyTestExcluded`. `reason` is the first reason recorded for
  the file; `otherReasons` holds the rest in the order of the table below. An import that left the
  analysis root is listed ahead of the helper refusal it causes.
  The reasons read from a test's own context are added to the file's list: `mock-factory-unreadable`
  and `fixture-unreadable` whenever a test has one (a test can have both), and `unresolved-context`
  (any other unresolved fact) only when no helper refusal already accounts for the excluded tests.
  The list covers the whole analyzed corpus, whatever `--changed-since` scopes the findings to.
- `testsTotal` is `0` for a file that was never parsed, and for a file whose tests all come from a
  refused helper: those tests are not counted anywhere. `everyTestExcluded` matches the files
  counted by `blindFiles`.
- The summary's `exclusions` counts every file by primary reason (`files`, `tests`, `omitted` and
  `reasons`, most files first), so it stays exact when the list is capped. The list holds at most
  10,000 files, keeping those with the most excluded tests; `omitted` counts the rest.
- On the terminal, stderr adds the five largest reasons with file and test counts, then "and N more
  kind(s)", and points at these reports. `html` and `sarif` carry `incompleteCauses` only.
- A file no project governs, and a file in an unsupported language, are deliberate classification
  rather than exclusions. They stay in the diagnostics and are not listed.

| Code | Meaning |
| --- | --- |
| `helper-export-unreadable` | An imported runner's export could not be resolved, read, or proved to be one function. |
| `helper-setup-effects` | An imported runner or a module on its chain has setup, fixture or module effects that were not proved inert. |
| `helper-effects-unproved` | A call, argument or parameter write inside an imported runner has unproved effects. |
| `helper-limit` | A helper expansion budget, recursion or depth limit was reached. |
| `helper-sources-unavailable` | Helper bodies are unavailable in a historical revision baseline. |
| `runner-mocked` | The runner, or a module on its re-export chain, is mocked or mocking is unresolved. |
| `unsupported-expression` | A title, callback, option or registration expression is outside the supported shapes. |
| `mock-factory-unreadable` | A `vi.mock` factory could not be read. |
| `fixture-unreadable` | A `test.extend` fixture value could not be read. |
| `unresolved-context` | Another ambient fact of a test could not be resolved: a dynamic title, an unreadable `.each` table or nested config. |
| `unsupported-statement` | A statement form the walker does not enter, such as `with`, may hide declarations. |
| `parse-error` | The parser reported errors. |
| `unsupported-source` | The source type is unsupported. |
| `source-limit` | The nesting or syntax-unit limit refused the file. |
| `extraction-failed` | Extraction could not run, or panicked and was recovered. |
| `walk-limit` | The per-file declaration or loop-unrolling budget was reached. |
| `import-outside-root` | An import resolved outside the analysis root and was not read. |
| `globals-undetermined` | The governing project's `globals` setting could not be determined. |
| `file-too-large` | The file exceeds the source-size limit and was not read. |
| `project-config-unreadable` | Project configuration could not be read, so no project could be matched and the file was skipped. |
| `unreadable` | The file could not be read. |
| `memory-budget` | The memory budget was reached before the file was analyzed. |

## Source limits

Source files are limited to 8 MiB. Before parsing, the frontend also refuses lexical bracket
nesting deeper than 256 and more than 8,192 raw syntax units.

- The units count opening brackets, operator, member, label and escape punctuation, and recursive
  control and type keywords, including occurrences in strings, comments and regexes.
- Nonnegative integer payloads and commas do not consume units.
- The bound is conservative: lexical ambiguity must not hide stack-exhausting input. A wide
  generated file or punctuation-heavy literal can therefore exceed it even when its actual syntax is
  shallow.
- These are analyzer resource limits, not JavaScript or Vitest restrictions.

Refused files make analysis incomplete and produce diagnostics; findings in other files remain
available. First-party imported helpers use the same preflight limits. Use
`--fail-on-incomplete=reads` (or `--fail-on-incomplete`) to make refused input an operational
failure. Admitted extraction runs on an explicitly sized worker stack; a larger stack alone would
not protect against unbounded input.

## Declaration limits

Each file emits at most 10,000 declarations (tests, `.each` rows and suite records, including those
from imported runners) and unrolls at most 10,000 literal `for...of` iterations. A `.each` table or
loop array longer than what remains of its limit is skipped without being read. Past either limit
the file stops emitting and counts as not read in full; the declarations already emitted are
compared as usual.

## Memory budget

The run reads whole files until the analyzer holds more than its memory budget: 2,048 MiB by
default, set with `--max-memory <MiB>` and removed with `--no-memory-cap`. The figure is the retained
heap `--debug-memory` reports plus source text kept for the HTML report.

A second backstop checks current process RSS after each file and stops above 1.5 times the budget on
Linux, macOS and Windows.

- RSS includes allocator overhead and other resident allocations; it is separate from the lifetime
  peak reported by `--debug-memory`.
- It is an absolute process measurement: it includes the startup footprint and memory still resident
  after the baseline pass, even after baseline data is dropped.
- A budget whose RSS ceiling is below that footprint can stop the current pass after its first
  file, even when little test data is retained.
- If the OS sample is unavailable, the retained-byte check still applies.
- `--no-memory-cap` disables both checks.

The checks run at file boundaries, so a single file or the later analysis phase can exceed either
threshold. Files after that point are not analyzed and are counted in `filesOverMemoryBudget`
(`json`, `jsonl`, `sarif` and the HTML island all carry it; the terminal and HTML banners also name
the cause). Every rule runs over the files read, with the same findings as a run over exactly those
files. `--baseline-from-ref` applies the same budget to the base revision.

Back to the [documentation index](README.md).
