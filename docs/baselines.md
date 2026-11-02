# Baselines and changed-file scoping

Both exist so a gate can act on what a change adds, not on what a codebase already holds. A run
that examined a subset says so in its output, because it is not a verdict on the whole repository.

## Baseline file

`--baseline <path>` suppresses findings the file already records, matching on `fingerprint`. Each
suppressed finding is marked `baselined` rather than removed, so a report still shows the full
picture while the gate counts only what is new.

`--update-baseline` atomically replaces that file from the current run, with the same
destination-symlink and failure handling as file reports. It is refused when the run is incomplete.

## Changed-file scoping

`--changed-since <rev>` reports only findings with a participant's declaration source or
represented caller in a file changed since that revision. Findings withheld this way force
`incomplete`: they are real, were found, and are not in this report.

Git-backed modes use local objects and ignore inherited `GIT_*` command/repository settings and
replacement refs. They retain file-based Git settings, including ignores, line endings and linked
worktrees. Commands do not refresh the index or fetch missing objects. Changed-file comparison does
not run filesystem monitors, external diffs or text conversions. If Git needs an executable
clean/process filter to compare a tracked file, the run fails with an operational error (exit 2);
it does not treat unconverted bytes as a valid comparison. Installed but unused filters do not
prevent analysis on supported Git versions. Configured executable filter names containing `=`
require Git 2.31 or newer for changed-file comparison; older Git fails explicitly. Historical
baselines read committed blobs without those conversions.

## Baseline from a revision

`--baseline-from-ref <rev>` computes the baseline from the base revision's own findings instead of a
file.

- The revision's files are read through `git cat-file`, never checked out, so no
  repository-controlled hook or filter runs.
- They are admitted by the same extension classifier and include/exclude globs as a working-tree
  run.
- Per-project `include` overrides and in-source candidates are not applied to the base read. That
  can only make the base report fewer findings, which makes the gate stricter, never looser.
- The base revision is subject to the same memory budget as the current run.
- The default engine refuses imported runners on the base read. See
  [Imported runners](imported-runners.md#baselines).
- Under the experimental next engine, project imports and resolver tsconfig/package metadata are
  read from the same pinned commit, with no working-tree fallback. Ancestor configuration is
  available within the repository; project-source imports remain confined to the analysis root.
  Current include/exclude globs and resolved globals configure both passes. Historical Vitest
  configuration is not executed.
- Historical workspace symlinks are not reconstructed. An unresolved import matching a committed
  project package name marks the historical pass incomplete and withholds its allowance. Names
  come from bounded committed metadata inside the analysis root; unreadable or exhausted identity
  metadata also makes the pass uncertain. Ordinary nonmatching external packages remain opaque.
  Unrecorded package aliases cannot be reconstructed.
- Missing unrelated blob sizes do not abort catalog construction. A required root, helper or
  configuration blob that is unavailable locally still fails explicitly without fetching.
- Repository roots must be UTF-8. Unqueryable names outside a nested analysis root do not abort
  catalog construction; the shared source reader's admission policy is unchanged.
- If the next engine cannot obtain a package-based tsconfig extension from Git, the historical
  pass is incomplete and contributes no baseline allowance. It never substitutes installed or
  current configuration. Current findings are still detected and gated normally. Missing relative
  configuration, escaped paths, malformed committed config, and failed committed reads remain errors. `#`-mapped
  tsconfig extensions are not included in this admission and remain explicit errors.
- The next engine refuses historical source/helper symlinks and actual dependency/configuration
  access failures. Oversized or invalid-UTF-8 root sources retain the shared reader's skip policy
  and explicit skip diagnostics. Its retained Git source index counts toward the memory budget;
  if the index alone exceeds the cap, the command fails with a memory-budget diagnostic.

## What a report records

`modes` records which of these applied, with counts: `baselineApplied`, `baselined`, `newFindings`,
`changedScope`, `withheldUnchanged`, and `suppressionsApplied`. A consumer never has to infer the
run's shape from the finding list.

A finding removed by an inline suppression directive is absent from the finding list entirely;
`suppressionsApplied` is what stops that absence from reading as a clean result.

Back to [Reports](reports.md).

`--fail-on-new N` works with either `--baseline FILE` or `--baseline-from-ref REVISION`.
It cannot be used with `--update-baseline`, and the two baseline sources cannot be combined.
