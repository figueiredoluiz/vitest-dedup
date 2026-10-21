#!/usr/bin/env node
// Corpus gate for fixtures/duplicates/ and fixtures/observed/ — plus, since V1, a whole-tree
// blindness ratchet over ALL of fixtures/ (see `DELIBERATE_BLIND_FILES`): the per-fixture
// expectations below cover three directories, while the blind set is pinned across the entire
// tree, because blindness can be manufactured anywhere a fixture's extraction can degrade.
//
// `fixtures/observed/` was added when the six pinned reference repositories moved to scheduled
// monitoring: each of its fixtures distils one shape actually found on that corpus into something
// this repository owns, reads in one screen, and can check offline. See `TARGETS` below. Both
// fixture directories are on the blocking merge path; the external reference repositories they
// were distilled from are not -- those are scheduled monitoring. Everything else in this file
// applies to both directories identically.
//
// Historically this gate inferred a fixture's expected verdict from its filename
// (`duplicates-*` "must fire", everything else "must stay silent") and only checked whether *any*
// finding mentioned the file. An external review found that structurally incapable of protecting
// the near-duplication corpus it was about to be handed:
//
//   - Every planned `near-*`/`parameterization-*` fixture would be read as expecting silence,
//     because the prefix check only recognized `duplicates-` and `duplicate-test-name`.
//   - "Some finding mentions this file" is satisfied by the wrong rule firing between the wrong
//     tests -- a `duplicate-test-name` warning would satisfy a near-duplication positive while the
//     rule under test found nothing at all.
//   - There was no way to tell "compared and found different" (`distinct`) from "never compared"
//     (`not-compared`); a fixture could pass because its tests were silently skipped.
//
// This version replaces the filename heuristic with a machine-readable expectation block in each
// fixture's header comment (see `CONTRIBUTING.md`'s fixture conventions for the verdict
// vocabulary). The block states, per fixture: how many tests must be extracted, and for every pair
// (or cluster edge) under test, which rule must fire, at what severity, naming which participants
// by file-relative line and title -- or, for a pair expected to produce no finding, whether that
// silence is `distinct` (compared, found different) or `not-compared` (never eligible).
//
// A fixture with no block, or a block that fails to parse, is a gate FAILURE, never a silent
// default -- see `parseFixture` below. This is deliberate: it is what lets another agent drop a
// new fixture into this directory without this gate quietly waving it through.
//
// ---------------------------------------------------------------------------------------------
// Expectation block format
// ---------------------------------------------------------------------------------------------
//
// Parsed by `lib/fixture-directives.mjs`, which owns the format and documents it in full. The short
// version: plain `//` comment lines carrying `@vitest-dedup:extracted`, `@vitest-dedup:expect`,
// `@vitest-dedup:isolates`, and `@vitest-dedup:participant`, stating per fixture how many tests must
// be extracted and, for every pair under test, which rule must fire at what severity between which
// participants -- or, for a pair expected to produce no finding, whether that silence is `distinct`
// (compared, found different) or `not-compared` (never eligible), and which single variable the pair
// isolates.
//
// That parser is shared with `check-fixture-isolation.mjs`, which this gate also runs (see the end
// of `main`). The two checks are complementary and neither is sufficient alone: this one asserts the
// analyzer stayed silent between the declared participants, and that one asserts the participants
// were close enough for the silence to mean something. A negative fixture whose participants differ
// in more than the one variable it claims to isolate passes this gate while proving nothing -- the
// silent-success shape the work-performed floor exists to reject.
//
// ---------------------------------------------------------------------------------------------
// distinct vs not-compared: what this gate can and cannot verify
// ---------------------------------------------------------------------------------------------
//
// No reporter exposes per-test comparison eligibility -- neither the JSON nor the JSONL report
// enumerates tests that did not appear in a finding, so there is no direct way to ask "was this
// specific pair ever eligible for comparison". What IS exposed is `AnalysisResult::incomplete`
// (`summary.incomplete` in both reports), and `analysis::analyze` sets it whenever ANY test in the
// run has an unresolved `Context` (dropped mock, unreadable `test.extend` factory, unresolvable
// `.each` table, dynamic title, opaque body -- see `model.rs`'s `Context::unresolved` and
// `analysis.rs`'s `has_unresolved_context`), independent of whether any rule fires.
//
// Run alone (this gate already isolates every fixture into its own directory to attribute
// extraction counts -- see `checkExtraction`), that per-file `incomplete` flag becomes a usable,
// if coarse, proxy:
//
//   - `not-compared` asserts the solo run's `incomplete` is `true`: something in the file was
//     genuinely unresolved, so absence of a finding proves nothing.
//   - `distinct` asserts the solo run's `incomplete` is `false`: nothing was silently excluded on
//     those grounds, so the tests were actually compared and found different.
//
// This only catches the `Context::unresolved` family of exclusions. A body excluded from
// `duplicate-test`/`parameterization-candidate` for being a recognized trivial stub
// (`Fingerprints::trivial`) does NOT set `incomplete`, so `summary.incomplete` alone cannot
// distinguish "trivial body, never compared" from "compared, found different". It no longer has
// to: `Fingerprints::trivial` is already on the `--emit-test-fingerprints` stream (`"test"` and
// `"hook"` records alike), so this gate runs that stream once per directory and uses it as a
// SECOND eligibility signal -- a `not-compared` verdict may also rest on a participant whose
// record is trivial (excluded from comparison for that reason, run complete or not), and a
// `distinct` verdict is rejected when a participant's record says trivial, because "compared and
// found different" is then false: the pair was never compared at all. A participant with no
// resolvable record (a suite, resolved through its children) asserts nothing here, the same way
// it asserts nothing in the isolation gate.
import { copyFileSync, existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, join } from 'node:path'
import { report as reportIsolation, run as runIsolation } from './check-fixture-isolation.mjs'
import { parseFixture } from './lib/fixture-directives.mjs'
import { findCallback, tokenize } from './lib/js-tokens.mjs'
import { defaultBinary, runAnalyzerJsonl } from './lib/run-analyzer.mjs'

const binary = process.argv[2] ?? defaultBinary()

// Every fixture directory this gate reads, with the rule overrides its run is analyzed under.
//
// `fixtures/observed/` is the newer half: each of its fixtures is a distilled, self-contained
// reproduction of a shape actually found on the pinned external corpus -- a distilled fixture can
// only pin a shape someone already found, which is why the corpus is monitored at all. The
// reference repositories moved to scheduled monitoring, so what was learned from them has to be
// pinned by fixtures this repository owns and can run offline -- which is worth nothing unless a
// gate reads them, hence this list rather than a second script.
//
// Both directories are analyzed with the same assertions (`parseFixture`, per-fixture extraction
// counts, solo-run eligibility, the unclaimed-finding sweep, and the isolation gate at the end),
// and now under the same `rules` as well.
//
// `subsumed-test` is enabled everywhere, and is the only override left. It ships Off on measured
// precision, and this gate pins rule *behavior* (which participants, which verdict, which severity
// when run) while the default itself is pinned by `tests/docs_contract.rs`'s severity contract
// against README's table. A rule whose default is a product decision must not become untested the
// moment that decision lands.
//
// `cross-file-duplicate` used to be the second entry, enabled for `fixtures/observed/` only,
// because `fixtures/duplicates/` held a pair whose declared contract was silence under the default
// config. Both halves of that arrangement are gone: the rule ships `Warning` on an exhaustive
// adjudication (9/9 `redundant` over 15,980 tests), and `fixtures/duplicates/` now declares eight
// real `cross-file-duplicate` positives -- the promoted `cross-file-duplicate-would-fire-{a,b}`
// pair plus the seven edges that other fixtures in that directory form with each other by sharing
// a body. Keeping the override would have been worse than redundant: with the rule on by default,
// `--rule cross-file-duplicate=warning` would keep every one of those expectations passing even if
// the shipped default were reverted, which is the exact shape of a gate that cannot fail.
const TARGETS = [
  { dir: 'fixtures/duplicates', rules: ['subsumed-test=warning'] },
  { dir: 'fixtures/observed', rules: ['subsumed-test=warning'] },
  // `fixtures/mixed` holds fixtures where TWO named defects interact in one file, so a fix for
  // one cannot be validated without showing it did not silence the other. Same overrides as the
  // other two directories -- no shipped default is masked by a flag -- and the empty-directory
  // floor in `main()` covers it, which matters because this directory was created empty and
  // stayed that way for a while.
  { dir: 'fixtures/mixed', rules: ['subsumed-test=warning'] },
]

/** `{file, line}` -> a stable string key, so location sets can be compared with `Set`/`Map`. */
const locKey = ({ file, line }) => `${file}:${line}`

/** Builds a `Map<locKey, {title, inert}>` from an expectation's or a finding's participants. */
function participantMap(participants) {
  const map = new Map()
  for (const p of participants) map.set(locKey(p), { title: p.title, inert: p.inert })
  return map
}

/** Whether two participant maps have exactly the same keys and, per key, the same title/inert. */
function mapsEqual(a, b) {
  if (a.size !== b.size) return false
  for (const [key, value] of a) {
    const other = b.get(key)
    if (!other || other.title !== value.title || other.inert !== value.inert) return false
  }
  return true
}

// Exit 0 (clean run) and exit 1 (error-severity findings exist, the expected case for this
// corpus) are both legitimate; anything else is an operational failure. See
// `lib/run-analyzer.mjs` for why this is a shared helper rather than a copy in each gate.
//
// The rule overrides come from the target being analyzed; see `TARGETS` above for which rules each
// directory enables and why they differ.
/**
 * Run the analyzer with the target-specific rule overrides and return its parsed JSONL
 * records.
 */
function runJsonl(target, args) {
  const rules = target.rules.flatMap((rule) => ['--rule', rule])
  return runAnalyzerJsonl(binary, [...args, ...rules]).records
}

/**
 * A file this gate enumerates as a fixture: named like a test, in any extension discovery reaches.
 *
 * This was `.endsWith('.test.ts')` and the narrowness was a blind spot, not a simplification. A
 * `.test.tsx` fixture placed in a scanned directory was COPIED into the temp run (so it analyzed and
 * could be named as a participant) but never enumerated, so its directive block was never parsed and
 * its expectations never asserted. It looked asserted and asserted nothing — the worst state for a
 * gate to be in, and invisible precisely because the gate is what would have reported it. Component
 * tests are roughly 8% of real test files, so the extension that mattered most was the one missing.
 *
 * Named like a test, not merely analyzable: a helper module beside a fixture carries no directive
 * block, and enumerating it would turn a legitimate sibling into a gate failure.
 */
const FIXTURE_NAME = /\.(test|spec|bench)\.[cm]?[jt]sx?$/

/**
 * Runs every check this gate owns against one fixture directory, appending to `failures`.
 *
 * @returns {number} how many fixtures the directory holds, for the summary line.
 */
function checkTarget(target, failures, pendingHeld) {
  const { dir } = target
  const fixtureNames = readdirSync(dir).filter((f) => FIXTURE_NAME.test(f)).sort()
  // Participant references may name any analyzable sibling, not only the fixtures this gate parses
  // and asserts: the directive grammar admits the full script-extension set discovery reaches, so a
  // cross-file pair with an in-source `.ts` half is nameable the day one lands. The enumeration
  // above is narrower than that on purpose -- it admits files NAMED like tests, because those are
  // the ones that can carry a directive block, and a helper module with no block would be a gate
  // failure rather than a fixture.
  const knownFiles = new Set([
    ...fixtureNames,
    ...readdirSync(dir).filter((f) => /\.[cm]?[jt]sx?$/.test(f)),
  ])

  // Parse every fixture's expectation block up front. A fixture that fails to parse still counts
  // toward the total and is reported, but is excluded from the finding/extraction checks below --
  // there is nothing well-formed left to check it against.
  const parsed = new Map()
  for (const name of fixtureNames) {
    const source = readFileSync(join(dir, name), 'utf8')
    const result = parseFixture(source, name, knownFiles)
    if (result.error) {
      failures.push(`FAIL ${join(dir, name)}: ${result.error}`)
    } else {
      parsed.set(name, result)
    }
  }

  // One whole-directory run: verdicts (especially the cross-file `-a`/`-b` pairs) depend on every
  // fixture being visible to the analyzer at once.
  const records = runJsonl(target, [dir])
  const findings = records.filter((r) => r.type === 'finding')
  const dirSummary = records.find((r) => r.type === 'summary')
  if (!dirSummary) {
    console.error(`FAIL ${dir}: whole-directory run produced no summary record`)
    process.exit(1)
  }

  // The triviality half of the eligibility check below, read off the fingerprint stream (one
  // extra run per directory, the same one `check-fixture-isolation.mjs` makes for itself). Only
  // hooks and tests get records; a participant naming a suite resolves nothing here, exactly as
  // in the isolation gate, and asserts nothing about triviality.
  const fingerprintRecords = runJsonl(target, [dir, '--emit-test-fingerprints'])
  const hooksByLine = new Map()
  const testsByTitle = new Map()
  for (const record of fingerprintRecords) {
    if (record.type === 'hook') {
      const key = `${basename(record.path)}\n${record.line}`
      if (!hooksByLine.has(key)) hooksByLine.set(key, [])
      hooksByLine.get(key).push(record)
    } else if (record.type === 'test') {
      const key = `${basename(record.path)}\n${record.title}`
      if (!testsByTitle.has(key)) testsByTitle.set(key, [])
      testsByTitle.get(key).push(record)
    }
  }
  const recordsFor = (participant) =>
    hooksByLine.get(`${participant.file}\n${participant.line}`) ??
    testsByTitle.get(`${participant.file}\n${participant.title}`) ??
    []
  const anyParticipantTrivial = (exp) =>
    exp.participants.some((p) => recordsFor(p).some((record) => record.trivial === true))

  // Normalize each finding to the same shape as a parsed expectation, so they can be compared with
  // `mapsEqual`. `path` in the report is forward-slash and directory-relative; only the basename
  // matters here since every fixture lives directly in `dir`.
  const normalizedFindings = findings.map((finding) => {
    const participants = finding.locations.map((loc, i) => ({
      file: basename(loc.path),
      line: loc.line,
      title: finding.titles[i],
      inert: finding.inert[i],
    }))
    return {
      rule: finding.rule,
      severity: finding.severity,
      participants,
      map: participantMap(participants),
    }
  })
  const claimed = new Set()

  // Per-fixture: extraction count and eligibility (via solo-run `incomplete`), then every declared
  // expectation against the whole-directory findings.
  for (const name of fixtureNames) {
    const fixture = parsed.get(name)
    if (!fixture) continue // already recorded as a parse failure above
    const path = join(dir, name)

    const solo = mkdtempSync(join(tmpdir(), 'vd-corpus-'))
    let soloSummary
    try {
      copyFileSync(join(dir, name), join(solo, name))
      soloSummary = runJsonl(target, [solo]).find((r) => r.type === 'summary')
    } finally {
      rmSync(solo, { recursive: true, force: true })
    }
    if (!soloSummary) {
      failures.push(`FAIL ${path}: solo run produced no summary record`)
      continue
    }
    if (soloSummary.testsAnalyzed !== fixture.extracted) {
      failures.push(
        `FAIL ${path}: expected ${fixture.extracted} extracted test(s), got ${soloSummary.testsAnalyzed}`,
      )
    }

    for (const exp of fixture.expectations) {
      const expectedMap = participantMap(exp.participants)
      const describeParticipants = () =>
        exp.participants.map((p) => `${p.file}:L${p.line} "${p.title}"`).join(', ')

      // A pending expectation is a one-way ratchet, the same shape as the isolation gate's
      // `isolation-violation`: it must NOT be satisfied today, and the gate fails the moment it
      // starts being satisfied, demanding the marker's removal so the expectation becomes a hard
      // assertion. This is what lets a spec-first fixture pin a KNOWN DEFECT (an extra finding the
      // analyzer should not produce, or a true finding it cannot produce yet) inside the corpus
      // instead of in a directory nothing asserts: while the defect is present the marker holds,
      // counted and printed below; the day the fix lands the gate goes red on purpose, because a
      // marker that outlives its fix is how "pending" decays into "unasserted".
      //
      // The distinct/not-compared eligibility assertions are skipped for pending expectations:
      // they describe the intended state, and until the fix lands only the finding-state drives
      // the ratchet. They come alive with the marker's removal, which the ratchet itself forces.
      if (exp.pending) {
        if (exp.rule !== 'none') {
          const matches = normalizedFindings.flatMap((f, index) =>
            f.rule === exp.rule && f.severity === exp.severity && mapsEqual(f.map, expectedMap)
              ? [index]
              : [],
          )
          if (matches.length > 0) {
            for (const index of matches) claimed.add(index)
            failures.push(
              `FAIL ${path}: pending expectation ${exp.rule} (${exp.verdict}, ${exp.severity}) ` +
                `between ${describeParticipants()} is already satisfied today -- remove the ` +
                `@vitest-dedup:pending marker so the finding becomes a hard expectation`,
            )
          } else {
            pendingHeld.push(`${join(dir, name)}: ${exp.rule} (${exp.verdict}) -- ${exp.pending}`)
          }
        } else {
          // Every finding linking these participants is this expectation's defect, not just the
          // first: an unrendered-title cluster can emit several findings whose participants are
          // indistinguishable by (line, title), and each must be claimed here or the
          // unclaimed-finding sweep below reports one this marker already accounts for.
          const offenders = normalizedFindings.flatMap((f, index) =>
            exp.participants.every((p) => {
              const found = f.map.get(locKey(p))
              return found && found.title === p.title
            })
              ? [index]
              : [],
          )
          if (offenders.length === 0) {
            failures.push(
              `FAIL ${path}: pending rule=none expectation (${exp.verdict}) between ` +
                `${describeParticipants()} is already satisfied today -- no finding links them, ` +
                `so the intended silence already holds. Remove the @vitest-dedup:pending marker ` +
                `so the silence becomes an assertion`,
            )
          } else {
            for (const index of offenders) claimed.add(index)
            pendingHeld.push(
              `${join(dir, name)}: ${offenders.length} ${normalizedFindings[offenders[0]].rule} ` +
                `finding(s) still link ${describeParticipants()} -- ${exp.pending}`,
            )
          }
        }
        continue
      }

      if (exp.rule !== 'none') {
        const matchIndex = normalizedFindings.findIndex(
          (f) => f.rule === exp.rule && f.severity === exp.severity && mapsEqual(f.map, expectedMap),
        )
        if (matchIndex === -1) {
          // Look for a finding between exactly these participants under a *different* rule or
          // severity, so the failure message says what actually happened rather than just "no".
          const sameParticipants = normalizedFindings.find((f) => mapsEqual(f.map, expectedMap))
          const actual = sameParticipants
            ? `${sameParticipants.rule} at ${sameParticipants.severity}`
            : 'no finding at all'
          failures.push(
            `FAIL ${path}: expected ${exp.rule} (${exp.verdict}, ${exp.severity}) between ` +
              `${describeParticipants()}; got ${actual}`,
          )
        } else {
          claimed.add(matchIndex)
        }
      } else {
        // distinct / not-compared: no finding may link these participants together.
        const offender = normalizedFindings.find((f) =>
          exp.participants.every((p) => {
            const found = f.map.get(locKey(p))
            return found && found.title === p.title
          }),
        )
        if (offender) {
          failures.push(
            `FAIL ${path}: expected no finding (${exp.verdict}) between ${describeParticipants()}; ` +
              `got ${offender.rule} at ${offender.severity}`,
          )
        }

        if (exp.verdict === 'not-compared' && !soloSummary.incomplete && !anyParticipantTrivial(exp)) {
          failures.push(
            `FAIL ${path}: verdict not-compared between ${describeParticipants()} requires the ` +
              `solo run to be incomplete (an unresolved context proving these were never eligible ` +
              `for comparison) or a participant judged trivial (excluded from comparison for that ` +
              `reason), but the solo run completed cleanly and every participant's record is ` +
              `non-trivial`,
          )
        }
        if (exp.verdict === 'distinct' && (soloSummary.incomplete || anyParticipantTrivial(exp))) {
          const why = soloSummary.incomplete
            ? 'the solo run was incomplete (something was excluded from comparison on unresolved-context grounds)'
            : 'a participant is judged trivial on the fingerprint stream -- a trivial body is never compared, so this silence is not-compared, not distinct'
          failures.push(
            `FAIL ${path}: verdict distinct between ${describeParticipants()} requires the solo ` +
              `run to complete without an unresolved context and every participant to have been ` +
              `eligible for comparison, but ${why}`,
          )
        }
      }
    }
  }

  // The tool-agnostic form of "a copy-paste detector would not flag these". The claim was
  // originally made about a named tool with no way to check it; pinning someone else's product
  // into this gate was refused, and what survives is the half this repository can actually
  // verify: every positive near-duplicate-test / subsumed-test expectation must link participants
  // whose RAW token streams differ. If the analyzer ever regressed to linking only what
  // token-level comparison could link, these expectations would fail here first -- the corpus,
  // not a slogan, carries the claim.
  const sources = new Map(fixtureNames.map((name) => [name, readFileSync(join(dir, name), 'utf8')]))
  for (const name of fixtureNames) {
    const fixture = parsed.get(name)
    if (!fixture) continue
    for (const exp of fixture.expectations) {
      // A pending expectation is not an asserted finding yet; its textual distinctness becomes a
      // claim worth checking the moment its marker comes off.
      if (exp.pending) continue
      if (exp.rule !== 'near-duplicate-test' && exp.rule !== 'subsumed-test') continue
      const streams = exp.participants.map((participant) => {
        const callback = findCallback(tokenize(sources.get(participant.file)), participant.line)
        if (callback.error) {
          throw new Error(`${name}: textual-distinctness check could not read a participant: ${callback.error}`)
        }
        return [...callback.header, ...callback.body].map((token) => token.value).join(' ')
      })
      const unique = new Set(streams)
      if (unique.size !== streams.length) {
        failures.push(
          `FAIL ${join(dir, name)}: a ${exp.rule} positive links token-identical participants -- ` +
          'if a token-level comparison would link them, this fixture proves nothing beyond it',
        )
      }
    }
  }

  // Every finding the analyzer produced must have been claimed by some fixture's expectation.
  // An unclaimed finding is either the wrong rule firing between a declared pair (already reported
  // above as a mismatch) or a genuinely new relationship no fixture accounts for -- both are
  // exactly the "a duplicate-test-name warning satisfies a near-duplication positive" class of
  // defect this gate exists to close.
  normalizedFindings.forEach((finding, index) => {
    if (claimed.has(index)) return
    const where = [...new Set(finding.participants.map((p) => p.file))].join(', ')
    failures.push(
      `FAIL ${where}: unexpected ${finding.rule} finding at ${finding.severity} between ` +
        `${finding.participants.map((p) => `${p.file}:L${p.line} "${p.title}"`).join(', ')} ` +
        `(not declared by any fixture's expectation block)`,
    )
  })

  return fixtureNames.length
}

// V1's remainder: the deliberate-blind ratchet, over the WHOLE fixture tree. Blind files --
// files whose every executable test was excluded -- are not a defect in the fixtures that carry
// them: each was written to exercise an exclusion (a tainted mock, a dynamic title, an unreadable
// table). What WOULD be a defect is a fixture going blind for a reason nobody chose, and the
// per-fixture `incomplete` expectations above cannot catch it: a fixture already `incomplete:
// true` for its deliberate cause can go blind on top without flipping anything the expectations
// read.
//
// The ratchet compares the SET, not the count (Copilot review, PR #32): a count can pass a swap
// -- one deliberate fixture un-blinding while another goes blind accidentally keeps 13 at 13.
// The set is derived from the analyzer's own per-test fingerprint stream (`--emit-test-
// fingerprints`, which filters to executable declarations exactly as `analyze` does), so the
// comparison needs no new report surface: group the stream's `test` records by path, and a file
// is blind when every record says `comparable: false`. The stream's own aggregate
// (`summary.blindFiles`) is cross-checked against the derived set for free -- the analyzer's
// count and the stream it emits must agree about the same corpus.
//
// The set is pinned in BOTH directions: a deliberate change (a fixture written to be blind, or
// one deliberately recovered) edits this list in the same commit; anything else fails with the
// path-level diff.
const DELIBERATE_BLIND_FILES = new Set([
  'declarations/template-literal-title-dynamic.test.ts',
  'duplicates/each-computed-key-row.test.ts',
  'duplicates/each-unresolved-table.test.ts',
  'duplicates/not-duplicate-each-row-loop-template-title.test.ts',
  'each/each-dollar-indexed.test.ts',
  'each/each-nonliteral-table.test.ts',
  'each/each-printf-directives.test.ts',
  'edge/vi-domock-scoped.test.ts',
  'edge/vi-dounmock-scoped.test.ts',
  'mixed/mixed-loop-each-in-non-idempotent-hook-scope.test.ts',
  'recall/duplicate-test-dynamic-title/session.test.ts',
  'recall/duplicate-test-name-dynamic-title/tenants.test.ts',
  'recall/mixed-loop-each-under-projects-entry/pkg/loops.spec.ts',
  // Framework-application shapes the analyzer cannot read yet: each file is blind on purpose until the
  // row that supports its shape lands, and leaves this list in the same change.
  'recall/framework-auto-import-in-helper/known.test.ts',
  'recall/framework-auto-import-in-helper/unknown.test.ts',
  'recall/framework-hook-helper-server/orders.test.ts',
  'recall/framework-mock-module-effects/alpha.test.ts',
  'recall/framework-mock-module-effects/beta.test.ts',
  'recall/framework-package-mock-call-uninstalled/users.test.ts',
  'recall/framework-package-mock-call/users.test.ts',
  'recall/framework-package-mock-factory-differs-negative/evening.test.ts',
  'recall/framework-package-mock-factory-differs-negative/morning.test.ts',
  'recall/framework-setup-helper-shapes/branch.test.ts',
  'recall/framework-setup-helper-shapes/foreach.test.ts',
  'recall/framework-setup-helper-shapes/globalcall.test.ts',
  'recall/framework-setup-helper-shapes/localconst.test.ts',
  'recall/framework-setup-helper-shapes/memberassign.test.ts',
  'recall/framework-setup-helper-shapes/objectkeys.test.ts',
  'recall/framework-setup-helper-shapes/spy.test.ts',
  // Default exports a runner must keep refusing (a `let`, a reassigned declaration, an imported
  // binding): blind on purpose, permanently, as the controls for the default-identifier support.
  'recall/framework-runner-default-imported-negative/slugs.test.ts',
  'recall/framework-runner-default-let-negative/slugs.test.ts',
  'recall/framework-runner-default-reassigned-negative/slugs.test.ts',
])

/**
 * Compare whole-corpus non-comparable test records with the pinned blind-file inventory,
 * appending any failures.
 */
function checkBlindFiles(binary, failures) {
  const { records } = runAnalyzerJsonl(binary, [
    'fixtures',
    '--emit-test-fingerprints',
  ])
  const summary = records.find((record) => record.type === 'summary')
  if (summary === undefined) {
    failures.push(
      'FAIL blind files: the whole-tree run produced no summary record, so blindness over the ' +
        'fixture corpus was not measured at all',
    )
    return
  }
  // Derive the blind set from the per-test stream: a file is blind when it emitted at least one
  // executable-test record and every one of them says not comparable.
  // Derive the blind set from the per-test stream. The accumulator tracks `anyComparable`
  // deliberately: a first version tracked `all: true` and cleared it on every non-comparable
  // record, which computes "ANY record was non-comparable" -- a file with four excluded rows
  // beside three compared tests read as blind. The stream-vs-summary cross-check below caught
  // it on its first run (14 derived vs 13 reported), which is that check earning its place.
  const comparableByPath = new Map()
  for (const record of records) {
    if (record.type !== 'test' || typeof record.path !== 'string') continue
    const entry = comparableByPath.get(record.path) ?? { any: false, anyComparable: false }
    entry.any = true
    if (record.comparable === true) entry.anyComparable = true
    comparableByPath.set(record.path, entry)
  }
  const blind = new Set(
    [...comparableByPath.entries()]
      .filter(([, { any, anyComparable }]) => any && !anyComparable)
      .map(([path]) => path),
  )

  // Free consistency check: the analyzer's own aggregate must equal the set its stream implies.
  const reported = summary.blindFiles
  if (!Number.isInteger(reported) || reported < 0) {
    failures.push(
      `FAIL blind files: summary field blindFiles is ${JSON.stringify(reported)}, not a ` +
        'non-negative integer; the analyzer and this gate disagree about the report schema',
    )
    return
  }
  if (reported !== blind.size) {
    failures.push(
      `FAIL blind files: the summary reports ${reported} but the test stream implies ` +
        `${blind.size}. The aggregate and the records it aggregates disagree -- an analyzer ` +
        'defect, not a corpus change',
    )
  }

  const missing = [...DELIBERATE_BLIND_FILES].filter((path) => !blind.has(path))
  const extra = [...blind].filter((path) => !DELIBERATE_BLIND_FILES.has(path))
  if (missing.length > 0 || extra.length > 0) {
    const detail = (label, paths) =>
      paths.length > 0 ? `\n  ${label}:\n${paths.sort().map((p) => `    ${p}`).join('\n')}` : ''
    failures.push(
      `FAIL blind files: the deliberate set drifted over fixtures/.${detail('no longer blind (recover the decision or update DELIBERATE_BLIND_FILES)', missing)}${detail('newly blind (a fixture went blind for a reason the per-fixture incomplete expectations cannot see)', extra)}`,
    )
  } else {
    console.log(
      `blind files: ${blind.size} deliberate, set-equal (pinned by path in DELIBERATE_BLIND_FILES)`,
    )
  }
}

/**
 * Check each owned fixture target and blind-file expectations, report held cases, and exit
 * nonzero when assertions fail.
 */
function main() {
  const failures = []
  const pendingHeld = []
  let total = 0
  // A floor on work performed: a directory that silently holds no fixtures would otherwise
  // contribute a clean pass over nothing, which is the
  // exact shape this corpus keeps re-learning. An empty (or renamed, or misspelled) target fails
  // here instead.
  for (const target of TARGETS) {
    const count = checkTarget(target, failures, pendingHeld)
    if (count === 0) {
      failures.push(
        `FAIL ${target.dir}: the directory holds no test-named fixtures. A scanned directory that ` +
          'contributes no assertions is indistinguishable from a passing one',
      )
    }
    total += count
  }

  // The whole-tree blindness ratchet runs BEFORE the failure-reporting block below, so a blind
  // drift is PRINTED, not just counted: wiring it in after the printer produced a silent exit 1
  // with no failure line -- a red that says nothing, caught by the revert-check.
  checkBlindFiles(binary, failures)

  let failed = false
  if (failures.length > 0) {
    for (const line of failures) console.error(line)
    console.error(`\n${failures.length} problem(s) across ${total} fixtures.`)
    failed = true
  } else {
    console.log(
      `ok: ${total} fixtures across ${TARGETS.map((t) => `${t.dir}/`).join(' and ')} matched their ` +
        `declared expectations (rule, verdict, participants, severity, extraction count).`,
    )
  }
  // Pending expectations are counted and printed on every run, not buried in fixture headers: each
  // one names a defect this corpus is holding open, and the count is what makes the ratchet above
  // observable. Unlike `fixtures/near/` below, these live inside asserted directories and their
  // expectation blocks are checked -- one gate failure away from becoming hard assertions.
  if (pendingHeld.length > 0) {
    console.log(
      `note: ${pendingHeld.length} pending expectation(s) held -- the defect each waits on is ` +
        'still present, and the gate fails the moment one starts being satisfied:',
    )
    for (const line of pendingHeld.sort()) console.log(`  ${line}`)
  }

  // The isolation gate runs here rather than as its own CI step, so that adding a fixture can never
  // satisfy one check while escaping the other. It answers the question this gate structurally
  // cannot: were the participants of a `rule=none` expectation ever close enough for the silence to
  // mean anything. Both run even when the first fails, because "the expectation block is wrong" and
  // "the fixture does not isolate what it claims" are independent defects and seeing only one of
  // them per CI run is how a second round of review finds the other.
  //
  // It takes the same binary this gate uses. It used to need none -- it compared fixture sources to
  // each other -- but it now reads the analyzer's own per-test fingerprints off
  // `--emit-test-fingerprints`, which is what made its translations exact rather than
  // approximations of them.
  if (reportIsolation(runIsolation(binary)) !== 0) failed = true

  if (failed) process.exit(1)
}

main()

// `fixtures/near/` holds fixtures whose expected verdict today's binary cannot produce: either an
// unimplemented rule, or -- since near-arrow-vs-function-this-twin.test.ts moved there -- a known
// analyzer defect being recorded honestly instead of being pinned as correct behavior. They are
// deliberately outside this gate's finding assertions (asserting them would fail against today's
// binary) but they are NOT outside the isolation gate, which needs no binary and checks them along
// with fixtures/duplicates/. A directory nothing reports on is how work gets forgotten, so count
// them and say so, every run.
const pendingDir = 'fixtures/near'
if (existsSync(pendingDir)) {
  const pending = readdirSync(pendingDir).filter((f) => FIXTURE_NAME.test(f))
  if (pending.length > 0) {
    console.log(
      `note: ${pending.length} fixture(s) in ${pendingDir}/ are pending an unimplemented rule or ` +
        'an analyzer fix, and their findings are not asserted by this gate:',
    )
    for (const name of pending.sort()) console.log(`  ${name}`)
  }
}
