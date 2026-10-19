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
| `json`, `jsonl` | The `incomplete` flag, plus `incompleteCauses`, which enumerates the reasons, counted. |
| `sarif` | `runs[0].invocations[0].properties.incomplete` and `incompleteCauses`, keyed as in the `json` report. |

`--fail-on-incomplete` turns the flag into a non-zero exit. A file in which every test was excluded
is a blind spot; see [Blind spots](#blind-spots).

### Choosing which causes count

`--fail-on-incomplete` or `--fail-on-incomplete=any` counts every cause, because the classes call
for different responses.

- `=reads` counts only causes meaning the project could not be fully read: unreadable, oversized or
  partially read files, undetermined config, and tests whose context could not be resolved.
- `=limits` counts only the analyzer's own bounded work being capped: the pair budget, the hook-scan
  budget, the memory budget, and truncated findings.

The value needs the `=`; with a space, `reads` would be read as the path.

### Blind spots

A file in which every test was excluded produced no findings, so it appears nowhere in the report.
Blind spots are named on stderr, worst first, counted in the `blindFiles` field of the `json` and
`jsonl` summaries, and gated by `--fail-on-incomplete=reads`, since a wholly excluded file is the
extreme form of an unresolved-context cause.

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
