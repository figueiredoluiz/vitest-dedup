#!/usr/bin/env node
// Unit coverage for `fixture-directives.mjs`, the single parser every `@vitest-dedup:` directive
// block in this repository flows through.
//
// The parser gates every fixture in `fixtures/{duplicates,observed,mixed,near}/`: a rule admitted
// by the wrong verdict, an `isolates` line accepted on a positive, or a participant regex that
// stops matching are each a way a fixture starts proving less than it claims, silently. Until this
// file existed the parser had no unit tests at all -- its behaviour was only exercised end to end
// by the corpus gates, which cannot tell "the grammar rejects this" from "no fixture tried".
//
// Run directly: `node --test scripts/check/lib/fixture-directives.test.mjs` (also part of
// `npm run test:check-lib`).
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import {
  ALL_RULES,
  enumerateFixtures,
  OFF_BY_DEFAULT_RULES,
  parseFixture,
  VERDICT_RULE,
} from './fixture-directives.mjs'

// Every parse in this file runs against the same fictional directory, so cross-file participant
// references and the extension set are exercised without touching a real fixture corpus.
const OWN_FILE = 'a.test.ts'
const KNOWN_FILES = new Set([
  'a.test.ts',
  'b.test.ts',
  'sibling.spec.js',
  'component.test.tsx',
  'weight.bench.ts',
  'units.ts',
  'legacy.test.mjs',
])

/** Parses `lines` (comment bodies, without the leading `//`) as one fixture's directives. */
function parse(lines) {
  const source = ['// fixture header prose', ...lines.map((line) => `// ${line}`)].join('\n')
  return parseFixture(source, OWN_FILE, KNOWN_FILES)
}

/** The smallest well-formed `rule=none` expectation: two participants, one isolation claim. */
const NEGATIVE = [
  '@vitest-dedup:extracted 2',
  '@vitest-dedup:expect rule=none verdict=distinct severity=none',
  '@vitest-dedup:isolates literal',
  '@vitest-dedup:participant L10 "first"',
  '@vitest-dedup:participant L20 "second"',
]

/** The smallest well-formed positive expectation. */
const POSITIVE = [
  '@vitest-dedup:extracted 2',
  '@vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error',
  '@vitest-dedup:participant L10 "first"',
  '@vitest-dedup:participant L20 "second"',
]

test('a minimal well-formed block parses to the documented shape', () => {
  const result = parse(NEGATIVE)
  assert.equal(result.error, undefined)
  assert.equal(result.extracted, 2)
  assert.equal(result.notRunnable, null)
  assert.equal(result.expectations.length, 1)
  const [exp] = result.expectations
  assert.equal(exp.rule, 'none')
  assert.equal(exp.verdict, 'distinct')
  assert.equal(exp.severity, 'none')
  assert.equal(exp.pending, null)
  assert.deepEqual(exp.isolates, {
    dimensions: ['literal'],
    unchecked: false,
    unstateable: false,
    notIsolable: false,
    reason: null,
  })
  assert.deepEqual(
    exp.participants.map((p) => [p.file, p.line, p.title, p.inert]),
    [
      [OWN_FILE, 10, 'first', false],
      [OWN_FILE, 20, 'second', false],
    ],
  )
})

// ---------------------------------------------------------------------------------------------
// The verdict/rule matrix, both directions
// ---------------------------------------------------------------------------------------------

test('every verdict accepts exactly its own rule set and rejects every other rule', () => {
  const allRules = new Set(
    Object.values(VERDICT_RULE)
      .filter((rules) => rules !== null)
      .flatMap((rules) => [...rules]),
  )
  for (const [verdict, allowed] of Object.entries(VERDICT_RULE)) {
    for (const rule of allRules) {
      const line = `@vitest-dedup:expect rule=${rule} verdict=${verdict} severity=error`
      const result = parse(['@vitest-dedup:extracted 2', line, ...POSITIVE.slice(2)])
      if (allowed === null) {
        // distinct / not-compared: rule=none is the only legal rule, at severity=none.
        assert.ok(result.error, `${verdict} x ${rule} must be rejected`)
        assert.match(result.error, /requires rule=none/)
      } else if (allowed.has(rule)) {
        assert.ok(!result.error, `${verdict} x ${rule} must parse: ${result.error ?? ''}`)
      } else {
        assert.ok(result.error, `${verdict} x ${rule} must be rejected`)
        assert.match(result.error, /requires one of /)
      }
    }
  }
})

test('an unknown rule, verdict, or severity is a parse error', () => {
  for (const bad of [
    'rule=telepathy verdict=redundant severity=error',
    'rule=duplicate-test verdict=redundantt severity=error',
    'rule=duplicate-test verdict=redundant severity=catastrophic',
  ]) {
    const result = parse(['@vitest-dedup:extracted 2', `@vitest-dedup:expect ${bad}`, ...POSITIVE.slice(2)])
    assert.ok(result.error, bad)
  }
})

test('rule=none requires severity=none and real rules reject severity=none', () => {
  const noneWithSeverity = parse([
    '@vitest-dedup:extracted 2',
    '@vitest-dedup:expect rule=none verdict=distinct severity=warning',
    ...NEGATIVE.slice(2),
  ])
  assert.match(noneWithSeverity.error, /requires severity=none/)

  const realWithoutSeverity = parse([
    '@vitest-dedup:extracted 2',
    '@vitest-dedup:expect rule=duplicate-test verdict=redundant severity=none',
    ...POSITIVE.slice(2),
  ])
  assert.match(realWithoutSeverity.error, /requires a real severity/)
})

test('an expect token without key=value, or a missing field, is a parse error', () => {
  for (const bad of ['rule', 'verdict=redundant severity=error', 'rule=duplicate-test verdict severity=error']) {
    const result = parse(['@vitest-dedup:extracted 2', `@vitest-dedup:expect ${bad}`, ...POSITIVE.slice(2)])
    assert.ok(result.error, bad)
  }
  const withReason = parse([
    '@vitest-dedup:extracted 2',
    '@vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error reason=regression',
    ...POSITIVE.slice(2),
  ])
  assert.equal(withReason.error, undefined)
  assert.equal(withReason.expectations[0].reason, 'regression')
})

// ---------------------------------------------------------------------------------------------
// isolates: required on rule=none, rejected elsewhere, dimension list hygiene
// ---------------------------------------------------------------------------------------------

test('a rule=none expectation without isolates is rejected', () => {
  const result = parse([
    '@vitest-dedup:extracted 2',
    '@vitest-dedup:expect rule=none verdict=distinct severity=none',
    ...NEGATIVE.slice(3),
  ])
  assert.match(result.error, /missing @vitest-dedup:isolates/)
})

test('isolates on a positive expectation is rejected', () => {
  const result = parse([...POSITIVE, '@vitest-dedup:isolates literal'])
  assert.match(result.error, /only meaningful on a rule=none expectation/)
})

test('isolates before any expect is rejected', () => {
  const result = parse(['@vitest-dedup:extracted 2', '@vitest-dedup:isolates literal', ...NEGATIVE.slice(1)])
  assert.match(result.error, /before any @vitest-dedup:expect/)
})

test('a duplicate isolates line for one expectation is rejected', () => {
  const result = parse([...NEGATIVE, '@vitest-dedup:isolates literal-kind'])
  assert.match(result.error, /duplicate @vitest-dedup:isolates/)
})

test('an unknown dimension is rejected with the known vocabulary listed', () => {
  const result = parse([
    ...NEGATIVE.slice(0, 2),
    '@vitest-dedup:isolates vibe',
    ...NEGATIVE.slice(3),
  ])
  assert.match(result.error, /unknown dimension "vibe"/)
})

test('an empty comma-separated dimension entry is rejected, not silently dropped', () => {
  const result = parse([...NEGATIVE.slice(0, 2), '@vitest-dedup:isolates literal,', ...NEGATIVE.slice(3)])
  assert.match(result.error, /empty dimension entry/)
  const leading = parse([...NEGATIVE.slice(0, 2), '@vitest-dedup:isolates ,literal', ...NEGATIVE.slice(3)])
  assert.match(leading.error, /empty dimension entry/)
})

test('a repeated dimension in one list is rejected', () => {
  const result = parse([...NEGATIVE.slice(0, 2), '@vitest-dedup:isolates literal,literal', ...NEGATIVE.slice(3)])
  assert.match(result.error, /repeats a dimension/)
})

test('a comma-separated dimension list parses whole', () => {
  const result = parse([...NEGATIVE.slice(0, 2), '@vitest-dedup:isolates identifier,literal', ...NEGATIVE.slice(3)])
  assert.equal(result.error, undefined)
  assert.deepEqual(result.expectations[0].isolates.dimensions, ['identifier', 'literal'])
})

// ---------------------------------------------------------------------------------------------
// the two opt-outs, and their mutual exclusion with isolation-violation
// ---------------------------------------------------------------------------------------------

test('unchecked, unstateable and not-isolable each require a reason naming the missing mechanism', () => {
  for (const sentinel of ['unchecked', 'unstateable', 'not-isolable']) {
    const bare = parse([...NEGATIVE.slice(0, 2), `@vitest-dedup:isolates ${sentinel}`, ...NEGATIVE.slice(3)])
    assert.match(bare.error, new RegExp(`${sentinel} requires a reason`))
    const reasoned = parse([
      ...NEGATIVE.slice(0, 2),
      `@vitest-dedup:isolates ${sentinel} the mechanism this claim needs does not exist yet`,
      ...NEGATIVE.slice(3),
    ])
    assert.equal(reasoned.error, undefined)
    const flag = { unchecked: 'unchecked', unstateable: 'unstateable', 'not-isolable': 'notIsolable' }[sentinel]
    assert.equal(reasoned.expectations[0].isolates[flag], true)
  }
})

test('an opt-out and an isolation-violation marker cannot both be set on one expectation', () => {
  const result = parse([
    ...NEGATIVE.slice(0, 2),
    '@vitest-dedup:isolates not-isolable the participants are identical by construction',
    '@vitest-dedup:isolation-violation something else also differs',
    ...NEGATIVE.slice(3),
  ])
  assert.match(result.error, /both opted out and marked isolation-violation/)
})

test('isolation-violation requires a reason and cannot repeat', () => {
  const bare = parse([...NEGATIVE.slice(0, 2), '@vitest-dedup:isolation-violation', ...NEGATIVE.slice(3)])
  assert.match(bare.error, /requires a reason/)
  const doubled = parse([
    ...NEGATIVE.slice(0, 2),
    '@vitest-dedup:isolation-violation first reason',
    '@vitest-dedup:isolation-violation second reason',
    ...NEGATIVE.slice(3),
  ])
  assert.match(doubled.error, /duplicate @vitest-dedup:isolation-violation/)
  const single = parse([
    ...NEGATIVE.slice(0, 3),
    '@vitest-dedup:isolation-violation hooks also differ',
    ...NEGATIVE.slice(3),
  ])
  assert.equal(single.error, undefined)
  assert.equal(single.expectations[0].isolationViolation, 'hooks also differ')
})

// ---------------------------------------------------------------------------------------------
// pending: per-expectation binding, the ratchet's input
// ---------------------------------------------------------------------------------------------

test('pending requires a reason', () => {
  const result = parse([...NEGATIVE.slice(0, 2), '@vitest-dedup:pending', ...NEGATIVE.slice(2)])
  assert.match(result.error, /requires a reason/)
})

test('pending before any expect is rejected: it marks one expectation, not the file', () => {
  const result = parse(['@vitest-dedup:extracted 2', '@vitest-dedup:pending waiting on a fix', ...NEGATIVE.slice(1)])
  assert.match(result.error, /before any @vitest-dedup:expect/)
  assert.match(result.error, /marks ONE expectation/)
})

test('a second pending line under one expectation is rejected', () => {
  const result = parse([
    ...NEGATIVE.slice(0, 2),
    '@vitest-dedup:pending waiting on a fix',
    '@vitest-dedup:pending another fix',
    ...NEGATIVE.slice(2),
  ])
  assert.match(result.error, /duplicate @vitest-dedup:pending/)
})

test('pending binds per expectation, so a live twin can sit beside a waiting pair', () => {
  const result = parse([
    '@vitest-dedup:extracted 2',
    ...NEGATIVE.slice(1),
    '@vitest-dedup:pending S5 fires today; remove when the idempotence fix lands',
    ...POSITIVE.slice(1),
  ])
  assert.equal(result.error, undefined)
  assert.equal(result.expectations[0].pending, 'S5 fires today; remove when the idempotence fix lands')
  assert.equal(result.expectations[1].pending, null)
})

// ---------------------------------------------------------------------------------------------
// not-runnable: file-level metadata
// ---------------------------------------------------------------------------------------------

test('not-runnable is file-level metadata and requires a reason', () => {
  const bare = parse(['@vitest-dedup:not-runnable', ...NEGATIVE])
  assert.match(bare.error, /requires a reason/)
  const beforeExpects = parse(['@vitest-dedup:not-runnable Vitest would refuse to run this file', ...NEGATIVE])
  assert.equal(beforeExpects.error, undefined)
  assert.equal(beforeExpects.notRunnable, 'Vitest would refuse to run this file')
})

// ---------------------------------------------------------------------------------------------
// extracted: exactly one, and an integer
// ---------------------------------------------------------------------------------------------

test('extracted must appear exactly once and be a non-negative integer', () => {
  assert.match(parse(NEGATIVE.slice(1)).error, /missing @vitest-dedup:extracted/)
  assert.match(parse([...NEGATIVE, '@vitest-dedup:extracted 3']).error, /duplicate .*extracted/)
  for (const bad of ['two', '-1', '2.5', '']) {
    const result = parse([`@vitest-dedup:extracted ${bad}`, ...NEGATIVE.slice(1)])
    assert.ok(result.error, `extracted "${bad}" must be rejected`)
    assert.match(result.error, /non-negative integer/)
  }
})

test('a block with no expectations at all is rejected', () => {
  const result = parse(['@vitest-dedup:extracted 1'])
  assert.match(result.error, /missing @vitest-dedup:expect block/)
})

// ---------------------------------------------------------------------------------------------
// participants: shape, flags, cross-file references, and the extension set
// ---------------------------------------------------------------------------------------------

test('an expectation with fewer than two participants is rejected', () => {
  const result = parse([...NEGATIVE.slice(0, 3)])
  assert.match(result.error, /fewer than 2 participants/)
})

test('a malformed participant line is rejected', () => {
  for (const bad of ['L10 first', '10 "first"', 'L0x "first"']) {
    const result = parse([...NEGATIVE.slice(0, 3), `@vitest-dedup:participant ${bad}`])
    assert.ok(result.error, `participant "${bad}" must be rejected`)
    assert.match(result.error, /malformed @vitest-dedup:participant/)
  }
})

test('the [inert] flag parses and any other flag is rejected', () => {
  const result = parse([...NEGATIVE.slice(0, 3), '@vitest-dedup:participant L10 "first" [inert]', '@vitest-dedup:participant L20 "second"'])
  assert.equal(result.error, undefined)
  assert.equal(result.expectations[0].participants[0].inert, true)
  assert.equal(result.expectations[0].participants[1].inert, false)
  const other = parse([...NEGATIVE.slice(0, 3), '@vitest-dedup:participant L10 "first" [stunned]', '@vitest-dedup:participant L20 "second"'])
  assert.match(other.error, /unknown participant flag/)
})

test('a cross-file reference must name a file the gate knows', () => {
  const result = parse([...NEGATIVE.slice(0, 3), '@vitest-dedup:participant b.test.ts#L10 "first"', '@vitest-dedup:participant L20 "second"'])
  assert.equal(result.error, undefined)
  assert.equal(result.expectations[0].participants[0].file, 'b.test.ts')
  const unknown = parse([...NEGATIVE.slice(0, 3), '@vitest-dedup:participant c.test.ts#L10 "first"', '@vitest-dedup:participant L20 "second"'])
  assert.match(unknown.error, /unknown file "c\.test\.ts"/)
})

test('the extension set admits every script suffix discovery reaches, not only .test.ts', () => {
  for (const file of ['sibling.spec.js', 'component.test.tsx', 'weight.bench.ts', 'units.ts', 'legacy.test.mjs']) {
    const result = parse([
      ...NEGATIVE.slice(0, 3),
      `@vitest-dedup:participant ${file}#L10 "first"`,
      '@vitest-dedup:participant L20 "second"',
    ])
    assert.equal(result.error, undefined, `${file} must be nameable: ${result.error ?? ''}`)
    assert.equal(result.expectations[0].participants[0].file, file)
  }
})

/**
 * The rule ids `Rule::id` returns, read from the Rust source: every `Self::Variant => "kebab-id"`
 * arm inside `pub fn id(&self)`. The match is exhaustive, so the arms ARE `Rule::ALL`'s names.
 */
function rustRuleIds(source) {
  const start = source.indexOf('pub fn id(&self)')
  assert.ok(start >= 0, 'src/model.rs no longer has `pub fn id(&self)`; the parity source moved')
  const end = source.indexOf('\n    }\n', start)
  assert.ok(end > start, 'src/model.rs: could not find the end of `Rule::id`; an unmatched marker would slice the whole file')
  const body = source.slice(start, end)
  return [...body.matchAll(/Self::\w+ => "([a-z-]+)"/g)].map((m) => m[1])
}

/** Rule ids whose `default_severity` match arm returns `Severity::Off`. */
function rustOffRuleIds(source) {
  const variants = new Map()
  const idStart = source.indexOf('pub fn id(&self)')
  const idEnd = source.indexOf('\n    }\n', idStart)
  for (const match of source.slice(idStart, idEnd).matchAll(/Self::(\w+) => "([a-z-]+)"/g)) {
    variants.set(match[1], match[2])
  }

  const severityStart = source.indexOf('pub fn default_severity(&self)')
  assert.ok(severityStart >= 0, 'src/model.rs no longer has `pub fn default_severity(&self)`')
  const severityEnd = source.indexOf('\n    }\n', severityStart)
  assert.ok(severityEnd > severityStart, 'src/model.rs: could not find the end of `Rule::default_severity`')
  const body = source.slice(severityStart, severityEnd)
  return [...body.matchAll(/Self::(\w+) => Severity::Off/g)].map((match) => {
    const id = variants.get(match[1])
    assert.ok(id, `default severity names unknown rule variant ${match[1]}`)
    return id
  })
}

test('the extractor reads every id arm and nothing else', () => {
  const ids = rustRuleIds(
    'impl Rule {\n    pub fn id(&self) -> &str {\n        match self {\n' +
      '            Self::A => "rule-a",\n            Self::B => "rule-b",\n        }\n    }\n' +
      '    pub fn other(&self) -> &str {\n        match self {\n            Self::A => "not-a-rule",\n        }\n    }\n}\n',
  )
  assert.deepEqual(ids, ['rule-a', 'rule-b'])
})

test('the JS rule table mirrors Rule::ALL exactly, so no Rust rule can escape the JS gates', () => {
  // `ALL_RULES` feeds the recall floor and the external-corpus retention floor. A rule added to
  // `Rule::ALL` without a verdict here would be invisible to both: neither floor could name it as
  // uncovered. The reverse -- a JS rule Rust no longer has -- would make a fixture expectation
  // parse for a rule that cannot fire. Both directions are one set comparison.
  const rust = rustRuleIds(readFileSync(new URL('../../../src/model.rs', import.meta.url), 'utf8'))
  assert.ok(rust.length >= 9, `premise: Rust has at least nine rules, read ${rust.length}`)
  assert.deepEqual(new Set(rust), new Set(ALL_RULES))
  assert.equal(new Set(rust).size, rust.length, 'Rule::id maps two variants to one id')
})

test('the JS off-by-default set mirrors Rust so supplemental retention cannot mask regressions', () => {
  const source = readFileSync(new URL('../../../src/model.rs', import.meta.url), 'utf8')
  assert.deepEqual(new Set(rustOffRuleIds(source)), OFF_BY_DEFAULT_RULES)
})

test('enumerateFixtures admits every test-named extension and keeps helpers as known siblings only', () => {
  const dir = mkdtempSync(join(tmpdir(), 'fixture-enum-'))
  try {
    const files = ['b.spec.tsx', 'a.test.ts', 'c.bench.mjs', 'd.test.cjs', 'helper.ts', 'notes.md', 'e.test.ts.bak']
    for (const name of files) writeFileSync(join(dir, name), '')
    mkdirSync(join(dir, 'sub.test.ts'))
    const { fixtureNames, knownFiles } = enumerateFixtures(dir)
    assert.equal(fixtureNames.length, 4, 'four test-named files are fixtures')
    assert.deepEqual(fixtureNames, ['a.test.ts', 'b.spec.tsx', 'c.bench.mjs', 'd.test.cjs'])
    assert.ok(knownFiles.has('helper.ts'), 'a helper sibling is a nameable participant file')
    assert.ok(!fixtureNames.includes('helper.ts'), 'a helper is never parsed as a fixture')
    assert.ok(!knownFiles.has('notes.md') && !knownFiles.has('e.test.ts.bak'))
    assert.ok(!knownFiles.has('sub.test.ts'), 'a directory named like a test is not a file')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
