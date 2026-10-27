#!/usr/bin/env node
// The single parser for the `@vitest-dedup:` expectation block carried in every fixture header.
//
// This lived inside `check-duplicates-corpus.mjs` until a second gate needed it. Copying it would
// have repeated the exact mistake `lib/run-analyzer.mjs` was created to undo -- the analyzer's
// exit-code contract was implemented once, copied, fixed in one copy, and silently regressed in the
// other, in a duplicate-detection tool. One parser, one place to review.
//
// ---------------------------------------------------------------------------------------------
// Directive format
// ---------------------------------------------------------------------------------------------
//
// Plain `//` comment lines, anywhere in the file (by convention, in the header, alongside the prose
// explaining *why* the fixture exists -- never replacing that prose):
//
//   // @vitest-dedup:extracted <N>
//   // @vitest-dedup:expect rule=<rule-id|none> verdict=<verdict> severity=<severity|none> [reason=<word>]
//   // @vitest-dedup:isolates <dimension>[,<dimension>...] | unchecked <reason> | unstateable <reason> | not-isolable <reason>
//   // @vitest-dedup:isolation-violation <reason>
//   // @vitest-dedup:participant [<other-file>#]L<line> "<full title>" [[inert]]
//   // @vitest-dedup:pending <reason>
//   // @vitest-dedup:not-runnable <reason>
//
// `extracted` appears exactly once per file: the number of tests the extractor must find when the
// file is analyzed alone.
//
// `expect` opens one expectation entry; every `isolates`/`participant` line that follows, up to the
// next `expect` (or end of file), belongs to it. A fixture needs one `expect` entry per finding it
// must produce, plus one per pair that must produce NO finding (`rule=none`).
//
// `isolates` is REQUIRED on every `rule=none` expectation and rejected on every other kind. It
// names the single variable the fixture claims to isolate, and is what
// `check-fixture-isolation.mjs` checks: a negative fixture that claims to isolate one variable must
// have participants whose fingerprints agree on everything else and disagree exactly where the
// analyzer is supposed to see that variable. Without it a negative fixture is a silent success
// waiting to happen -- it can stop discriminating and still pass forever
// (the work-performed floor every gate in this repository must assert).
//
// There are THREE opt-outs, and they are not interchangeable. All are explicit, counted and
// printed on every run; none is ever a default, because a default is how an unclassified fixture
// gets waved through.
//
//   `unchecked <reason>`     the expectation IS an isolation claim -- "these agree on everything
//                            but X" -- that the current surface cannot state. Debt: the reason must
//                            name the missing mechanism, and this count should go to zero.
//   `unstateable <reason>`   the claim's EQUAL half needs a view the fingerprint stream does not
//                            emit (near-duplicate phrasing-equivalence is the worked example), so
//                            no relation over the current stream can state it. Not debt at this
//                            surface: the reason must name the missing view and the analyzer event
//                            that produces it, and the expectation restates as a dimension when
//                            that event lands.
//   `not-isolable <reason>`  the expectation is not an isolation claim at all and never will be:
//                            its participants are identical in every emitted field and the silence
//                            is a policy decision; or the fixture's own content is that the
//                            analyzer cannot tell them apart; or the property is over a cluster, or
//                            stated relative to a different fixture. Permanent, and NOT debt.
//
// Keeping them apart is the point. Merged, the single number reads as 49% unverified debt when
// most of it is a permanent property of what those fixtures are for, and the genuinely-missing
// mechanisms disappear into it. `lib/isolation.mjs` documents exactly which shapes may claim
// `unstateable` and `not-isolable`.
//
// `participant` names one test in the pair/cluster-edge this expectation is about, by line (in THIS
// file, as it appears in the finished file, block included) and full title (suite path joined with
// " > ", matching `TestCase::full_title`). A pair can span two fixture files (the `-a`/`-b` corpus
// convention): prefix the line with the other file's bare name and `#`. Appending `[inert]` after
// the title asserts that participant is reported `.skip`/`.todo` in the finding.
//
// `isolation-violation` records that a fixture's declared `isolates` claim does NOT currently hold:
// its participants differ in more than the one variable it names, so the guard it exists to protect
// is not actually under test. It is a listed, reasoned, counted exception -- printed on every run --
// and it ratchets in one direction only: if a marked expectation starts PASSING the isolation check,
// the gate fails and demands the marker be removed. That is what keeps it from becoming the dumping
// ground a suppression list usually becomes. It is not interchangeable with `unchecked`:
// `unchecked` means "this gate cannot express the claim", `isolation-violation` means "this gate
// expressed the claim and the fixture does not meet it".
//
// `pending` marks ONE expectation -- the `expect` entry it sits directly under -- whose true verdict
// today's binary cannot produce (an unimplemented rule, or a known defect being pinned honestly
// rather than papered over). It binds per expectation, not per file, because a fixture may wait on
// a fix for one pair while asserting a live twin beside it (`fixtures/mixed/` exists for exactly
// that shape). `check-duplicates-corpus.mjs` reads it as a one-way ratchet, the same shape as
// `isolation-violation`: a pending expectation must NOT be satisfied today, and the gate fails the
// moment one starts being satisfied, demanding the marker's removal so the expectation becomes a
// hard assertion. A pending expectation therefore pins the *defect* (still present, counted,
// printed) instead of pinning the bug as correct behaviour. `not-runnable` marks a fixture that
// could not execute under real Vitest even though it analyzes correctly, so the open idea of
// running fixtures under Vitest as an oracle has a machine-readable exclusion list instead of a
// discovery process. It carries a free-text reason and stays file-level metadata: no gate asserts
// on it, but it is parsed here so a gate that wants it does not have to invent a second parser.
import { readdirSync } from 'node:fs'
import { DIMENSIONS, NOT_ISOLABLE, UNCHECKED, UNSTATEABLE } from './isolation.mjs'

const RULES = new Set([
  'duplicate-test',
  'duplicate-test-name',
  'parameterization-candidate',
  'near-duplicate-test',
  'cross-file-duplicate',
  'subsumed-test',
  'duplicate-suite',
  'duplicate-each-row',
  'redundant-hook',
  'none',
])
const SEVERITIES = new Set(['error', 'warning', 'off', 'none'])

/**
 * A file that carries a directive block: named like a test, in any extension discovery reaches.
 *
 * Narrowing this to `.test.ts` was a blind spot, not a simplification. A `.test.tsx` fixture was
 * copied into the temporary run (so it analyzed and could be named as a participant) but never
 * enumerated, so its expectations were never asserted. A helper module beside a fixture carries no
 * block and is not a fixture.
 */
export const FIXTURE_NAME = /\.(test|spec|bench)\.[cm]?[jt]sx?$/

/**
 * Lists one fixture directory the way every gate must: the same files, parsed and referenced
 * identically, so one gate cannot assert a fixture another never sees.
 *
 * Only regular files count; a directory named like a test would otherwise be read as a fixture.
 *
 * @param {string} dir fixture directory, flat.
 * @returns {{fixtureNames: string[], knownFiles: Set<string>}} the sorted fixtures to parse, and
 *   every analyzable file a participant may name (fixtures plus helper siblings).
 */
export function enumerateFixtures(dir) {
  const names = readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => entry.name)
  const fixtureNames = names.filter((name) => FIXTURE_NAME.test(name)).sort()
  const knownFiles = new Set([...fixtureNames, ...names.filter((name) => /\.[cm]?[jt]sx?$/.test(name))])
  return { fixtureNames, knownFiles }
}

/** verdict -> the set of rules it may pair with (`null` means `rule=none`). */
export const VERDICT_RULE = {
  // `redundant` covers every rule whose remedy is "one side can go" without merging: the exact
  // pair, the same pair across two files, a whole duplicated suite, a repeated table row, and a
  // hook that repeats an ancestor.
  //
  // `cross-file-duplicate` was missing from this set until a review noticed what that cost: it is
  // the same alpha-equal body under an equivalent context that `duplicate-test` proves redundant,
  // only without the single-file restriction, so no verdict accepting it meant
  // `@vitest-dedup:expect rule=cross-file-duplicate ...` was a parse error at any severity. While
  // the rule shipped Off that left it with no asserted positive anywhere in the corpus -- only
  // fixtures pinning its silence under the default config, which is also what a deleted rule
  // produces. Admitting it here is what later made the default flip assertable: the rule now ships
  // `Warning`, and both `fixtures/observed/` and `fixtures/duplicates/` declare positives for it
  // under the plain default config, with no `--rule` override anywhere.
  redundant: new Set([
    'duplicate-test',
    'cross-file-duplicate',
    'duplicate-suite',
    'duplicate-each-row',
    'redundant-hook',
  ]),
  consolidatable: new Set(['parameterization-candidate']),
  'name-collision': new Set(['duplicate-test-name']),
  // `similar-uncertain` is deliberately its own verdict rather than folded into `redundant`:
  // a near-duplicate is a review prompt, and claiming redundancy would assert an equivalence the
  // analyzer suspects but has not established. `subsumed-test` earns the same caution for the
  // same reason -- a strict subset of behavior events suggests the smaller test adds nothing,
  // but only a human can confirm the extra observations are irrelevant.
  'similar-uncertain': new Set(['near-duplicate-test', 'subsumed-test']),
  distinct: null,
  'not-compared': null,
}

/**
 * Every rule the analyzer implements, derived from `VERDICT_RULE` rather than listed again: that
 * object is this repository's single JS-side mirror of `Rule::ALL` (a rule no verdict admits
 * cannot be named by any `@vitest-dedup:expect`), so a rule arriving there reaches every gate
 * importing this -- recall coverage and external-corpus retention -- at once, instead of each
 * gate keeping a copy that can disagree on what "every rule" means. The mirror itself is held to
 * `Rule::id` in `src/model.rs` by a parity test in `fixture-directives.test.mjs`: a rule added
 * on the Rust side without a verdict here fails that test, not the retention floor silently.
 */
export const ALL_RULES = new Set(
  Object.values(VERDICT_RULE)
    .filter((rules) => rules !== null)
    .flatMap((rules) => [...rules]),
)

// The rules whose Rust `Rule::default_severity` is `Off`. External-corpus retention may enable
// only these in a supplemental run: retrying a default-enabled rule could hide a real regression
// in the shipped configuration. The Rust/JS parity test beside the rule-id parity test holds this
// deliberately small mirror to `src/model.rs`.
export const OFF_BY_DEFAULT_RULES = new Set(['subsumed-test'])

const DIRECTIVE_RE =
  /^\s*\/\/\s*@vitest-dedup:(extracted|expect|participant|isolates|isolation-violation|pending|not-runnable)\b\s*(.*?)\s*$/
// The cross-file reference (`other-file#`) admits every script extension discovery can reach, not
// just `.test.ts` -- the same `[cm]?[jt]sx?` class `lib/recall-runner.mjs`'s SOURCE_RE uses. A `.spec.js`
// or in-source `.ts` participant could never be named under the old `.test\.ts`-only shape, which
// made a cross-file expectation over one unstateable (the `duplicate-test-bench` and in-source
// pairs). The named file must still exist in the fixture directory (`knownFiles`), so widening the
// grammar widens what can be *expressed*, not what is silently accepted.
const PARTICIPANT_RE =
  /^(?:([A-Za-z0-9._-]+\.(?:[cm]?[jt]sx?))#)?L(\d+)\s+"([^"]*)"(?:\s*\[(\w+)\])?$/

/**
 * Parses one fixture's expectation block out of its raw source text.
 *
 * Returns `{ extracted, expectations, notRunnable }` on success, where each expectation is
 * `{ rule, verdict, severity, reason, isolates, isolationViolation, pending, participants }`,
 * `isolates` is `{ dimensions, unchecked, unstateable, notIsolable, reason }` or `null` for a
 * positive expectation,
 * `pending` is the per-expectation marker's reason or `null` (see the directive-format notes
 * above for the ratchet `check-duplicates-corpus.mjs` runs on it), and each participant is
 * `{ file, line, title, inert }` (`file` defaults to `ownFile`). Returns `{ error }` on any
 * malformed, missing, or internally inconsistent block -- there is no partial-success case, so an
 * unparseable fixture is a gate failure rather than a default.
 *
 * @param {string} source raw fixture text.
 * @param {string} ownFile the fixture's own bare filename.
 * @param {Set<string>} knownFiles every fixture filename a participant may reference.
 */
export function parseFixture(source, ownFile, knownFiles) {
  let extracted = null
  let notRunnable = null
  const expectations = []
  let current = null

  for (const raw of source.split('\n')) {
    const match = DIRECTIVE_RE.exec(raw)
    if (!match) continue
    const [, kind, rest] = match

    if (kind === 'not-runnable') {
      if (rest === '') return { error: `@vitest-dedup:${kind} requires a reason` }
      notRunnable = rest
      continue
    }

    if (kind === 'pending') {
      if (!current) {
        return {
          error:
            `@vitest-dedup:pending appears before any @vitest-dedup:expect. It marks ONE ` +
            `expectation -- the defect it waits on -- not the file, because a fixture may assert ` +
            `a live twin beside the pair that cannot pass yet. Place it directly under the ` +
            `expect line it belongs to`,
        }
      }
      if (rest === '') return { error: `@vitest-dedup:pending requires a reason` }
      if (current.pending) return { error: `duplicate @vitest-dedup:pending for one expectation` }
      current.pending = rest
      continue
    }

    if (kind === 'extracted') {
      if (extracted !== null) return { error: `duplicate @vitest-dedup:extracted directive` }
      if (!/^\d+$/.test(rest)) {
        return { error: `@vitest-dedup:extracted is not a non-negative integer: "${rest}"` }
      }
      extracted = Number(rest)
      continue
    }

    if (kind === 'expect') {
      const fields = {}
      for (const token of rest.split(/\s+/).filter(Boolean)) {
        const eq = token.indexOf('=')
        if (eq < 0) {
          return { error: `malformed @vitest-dedup:expect token (expected key=value): "${token}"` }
        }
        fields[token.slice(0, eq)] = token.slice(eq + 1)
      }
      const { rule, verdict, severity, reason } = fields
      if (!rule || !verdict || !severity) {
        return { error: `@vitest-dedup:expect is missing rule/verdict/severity: "${rest}"` }
      }
      if (!RULES.has(rule)) return { error: `@vitest-dedup:expect has unknown rule "${rule}"` }
      if (!(verdict in VERDICT_RULE)) {
        return { error: `@vitest-dedup:expect has unknown verdict "${verdict}"` }
      }
      if (!SEVERITIES.has(severity)) {
        return { error: `@vitest-dedup:expect has unknown severity "${severity}"` }
      }
      const expectedRules = VERDICT_RULE[verdict]
      if (expectedRules === null) {
        if (rule !== 'none') return { error: `verdict "${verdict}" requires rule=none, got rule=${rule}` }
        if (severity !== 'none') {
          return { error: `verdict "${verdict}" requires severity=none, got severity=${severity}` }
        }
      } else {
        if (!expectedRules.has(rule)) {
          return {
            error:
              `verdict "${verdict}" requires one of ${[...expectedRules].join(', ')}, got ` +
              `rule=${rule}`,
          }
        }
        if (severity === 'none') {
          return { error: `verdict "${verdict}" requires a real severity, got severity=none` }
        }
      }
      current = {
        rule,
        verdict,
        severity,
        reason: reason ?? null,
        isolates: null,
        isolationViolation: null,
        pending: null,
        participants: [],
      }
      expectations.push(current)
      continue
    }

    if (kind === 'isolates') {
      if (!current) return { error: `@vitest-dedup:isolates appears before any @vitest-dedup:expect` }
      if (current.rule !== 'none') {
        return {
          error:
            `@vitest-dedup:isolates is only meaningful on a rule=none expectation (this one is ` +
            `rule=${current.rule}); a fixture that must produce a finding is checked by the ` +
            `finding, not by an isolation claim`,
        }
      }
      if (current.isolates) return { error: `duplicate @vitest-dedup:isolates for one expectation` }
      for (const [sentinel, field] of [
        [UNCHECKED, 'unchecked'],
        [UNSTATEABLE, 'unstateable'],
        [NOT_ISOLABLE, 'notIsolable'],
      ]) {
        if (!rest.startsWith(`${sentinel} `) && rest !== sentinel) continue
        const reason = rest.slice(sentinel.length).trim()
        if (reason === '') {
          return {
            error:
              `@vitest-dedup:isolates ${sentinel} requires a reason -- an unexplained opt-out is ` +
              `the silent success this directive exists to prevent`,
          }
        }
        current.isolates = {
          dimensions: [],
          unchecked: field === 'unchecked',
          unstateable: field === 'unstateable',
          notIsolable: field === 'notIsolable',
          reason,
        }
        break
      }
      if (current.isolates) continue
      if (rest === '') {
        return {
          error:
            `@vitest-dedup:isolates requires a dimension, "${UNCHECKED} <reason>", ` +
            `"${UNSTATEABLE} <reason>", or "${NOT_ISOLABLE} <reason>"`,
        }
      }
      const dimensions = rest.split(',').map((part) => part.trim())
      if (dimensions.some((dimension) => dimension === '')) {
        return {
          error:
            `@vitest-dedup:isolates "${rest}" in ${ownFile} has an empty dimension entry (a stray ` +
            `or trailing comma) -- every comma-separated entry must name a real dimension, or the ` +
            `list silently narrows to fewer variables than declared and, if it narrows to none, ` +
            `reaches combineDimensions([]) with nothing to check`,
        }
      }
      for (const dimension of dimensions) {
        if (!DIMENSIONS.has(dimension)) {
          return {
            error:
              `@vitest-dedup:isolates has unknown dimension "${dimension}" (known: ` +
              `${[...DIMENSIONS.keys()].join(', ')}, or "${UNCHECKED} <reason>" / ` +
              `"${UNSTATEABLE} <reason>" / "${NOT_ISOLABLE} <reason>")`,
          }
        }
      }
      if (new Set(dimensions).size !== dimensions.length) {
        return { error: `@vitest-dedup:isolates repeats a dimension: "${rest}"` }
      }
      current.isolates = {
        dimensions,
        unchecked: false,
        unstateable: false,
        notIsolable: false,
        reason: null,
      }
      continue
    }

    if (kind === 'isolation-violation') {
      if (!current) {
        return { error: `@vitest-dedup:isolation-violation appears before any @vitest-dedup:expect` }
      }
      if (current.rule !== 'none') {
        return { error: `@vitest-dedup:isolation-violation is only meaningful on a rule=none expectation` }
      }
      if (current.isolationViolation) {
        return { error: `duplicate @vitest-dedup:isolation-violation for one expectation` }
      }
      if (rest === '') {
        return { error: `@vitest-dedup:isolation-violation requires a reason naming what else differs` }
      }
      current.isolationViolation = rest
      continue
    }

    // kind === 'participant'
    if (!current) return { error: `@vitest-dedup:participant appears before any @vitest-dedup:expect` }
    const pMatch = PARTICIPANT_RE.exec(rest)
    if (!pMatch) return { error: `malformed @vitest-dedup:participant: "${rest}"` }
    const [, file, line, title, flag] = pMatch
    if (flag && flag !== 'inert') return { error: `unknown participant flag "[${flag}]"` }
    const participantFile = file ?? ownFile
    if (!knownFiles.has(participantFile)) {
      return { error: `@vitest-dedup:participant references unknown file "${participantFile}"` }
    }
    current.participants.push({
      file: participantFile,
      line: Number(line),
      title,
      inert: flag === 'inert',
    })
  }

  if (extracted === null) return { error: `missing @vitest-dedup:extracted directive` }
  if (expectations.length === 0) {
    return { error: `missing @vitest-dedup:expect block (no expectations declared)` }
  }
  for (const expectation of expectations) {
    if (expectation.participants.length < 2) {
      return {
        error: `expectation rule=${expectation.rule} verdict=${expectation.verdict} has fewer than 2 participants`,
      }
    }
    if (
      expectation.isolationViolation &&
      (expectation.isolates?.unchecked ||
        expectation.isolates?.unstateable ||
        expectation.isolates?.notIsolable)
    ) {
      return {
        error:
          `expectation verdict=${expectation.verdict} is both opted out and marked ` +
          `isolation-violation. Those are different claims: an opt-out says the check cannot be ` +
          `expressed (or that there is no isolation claim to express), a violation says it WAS ` +
          `expressed and the fixture failed it. Pick one`,
      }
    }
    if (expectation.rule === 'none' && !expectation.isolates) {
      return {
        error:
          `expectation verdict=${expectation.verdict} between ` +
          `${expectation.participants.map((p) => `${p.file}:L${p.line}`).join(' and ')} is missing ` +
          `@vitest-dedup:isolates. A rule=none fixture passes by nothing happening, so without a ` +
          `declared isolated variable there is nothing asserting it still discriminates`,
      }
    }
  }
  return { extracted, expectations, notRunnable }
}
