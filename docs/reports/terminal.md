# Terminal report

## Layout

The terminal report starts with the tool version and groups findings by rule, ordered by
first-participant file within each rule. Blank lines separate rule headings and findings.

- Each finding shows its severity and message, followed by a `-->` primary location and `related:`
  locations. Every location retains its full test title and `path:line:column`.
- Baselined findings remain visible with a `[baselined]` marker and their original severity.
- Incomplete-run and changed-file warnings appear before findings.

Each finding ends with a rule-specific `action:` line:

- Name collisions call for renaming.
- Parameterization preserves every case and assertion.
- Similarity and subsumption require review before changing tests.
- Exact duplicates prompt review of removal or repair of intended coverage; cross-file duplication
  may be deliberate.

Actions also remain visible for baselined findings. A rule configured at severity `off` is not
computed, so a CLI run never reports a finding for it.

## Footer

The footer summarizes analyzed tests and files, displayed severities, inline suppressions, and
baseline debt. Truncated reports label shown findings and omitted counts explicitly. The severity
summary keeps its fixed `errors, warnings, off` shape, so the `off` count of a CLI run is always 0.

The CLI also shows the existing error-level duplication threshold: unique unbaselined error finding
locations divided by analyzed executable tests. Locations may belong to suites or hooks too, so the
numerator is labeled as locations rather than tests. Zero executable tests have no rate to display,
even when a suite finding fails policy.

`Analysis policy` is separate from the threshold and includes `--fail-on-new` and the selected
incomplete-run policy. `PASS` can be qualified by incomplete coverage; it does not certify the whole
repository. This is the analysis decision before report writing: a later output error still exits
operationally.

Library `write_report` omits threshold and policy labels; callers can supply
`TerminalReportContext` to `write_report_with_context`.

## Detection time

The CLI appends `Detection time: N.N ms` after the finding count, matching the sum of monotonic
discovery, primary parsing/extraction and analysis durations.

- Discovery includes project resolution and globals detection.
- Baseline preparation (including a ref-baseline analysis), baseline writes and report
  generation/I/O are excluded.
- `--no-metrics` gives byte-stable terminal output across equivalent runs. The Action exposes the
  same `no-metrics` input (default `false`). It is a CLI presentation option, not a
  configuration-file key.
- Machine reports contain no timing fields and are unchanged by the flag.
- Library reports omit timing unless the caller supplies `DetectionTiming` through
  `TerminalReportContext::with_timing`.

## Color

Color is enabled only when stdout is a terminal. Set `NO_COLOR` to any nonempty value to disable it;
an empty value allows color.

- Rule headings are bold cyan, error and warning labels red and yellow, locations blue, and summary
  counts gray.
- A library caller can render a finding at severity `off`; its label is gray. The CLI never
  produces one.
- Piped output and machine or file reports contain no tool-added ANSI styles.
- Library reporters default to plain text; `TerminalReporter { no_color: false }` explicitly enables
  styles.
- On Windows, the CLI enables virtual-terminal processing on stdout before adding styles; if console
  access or enabling fails, it uses plain text instead. Opting out of color and machine-only
  reporting do not change console modes.

Back to [Reports](../reports.md).
