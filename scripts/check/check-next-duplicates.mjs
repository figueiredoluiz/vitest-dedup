#!/usr/bin/env node
// The next engine's duplicate rules against the owned corpus: the spike's evaluator as a gate.
//
// The corpus gate pins the default engine, whose nine rules this repository ships. Under
// `VITEST_DEDUP_ENGINE=next` exactly two of them run (`duplicate-test`,
// `cross-file-duplicate`), and the fixture headers cannot say what that engine should claim --
// the headers are the default engine's vocabulary. This gate runs the next engine over the
// three fixture directories, reads the same expectation blocks, and asserts the ported rules'
// pairwise claims: every required pair linked, every declared silence kept, every delta
// adjudicated in `next-duplicates-known.json` with a reason. A rule the engine does not port
// reporting a finding is a failure outright, and floors on fixtures, tests analyzed and
// required pairs keep an empty read from passing.
//
// `lib/next-duplicates.mjs` owns every judgment, so each rejection is reachable from a unit
// test with a fabricated observation; this file is the run loop and the floors.
import { readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { enumerateFixtures, parseFixture } from './lib/fixture-directives.mjs'
import { runAsGate } from './lib/gate-main.mjs'
import { defaultBinary, runAnalyzerJsonl } from './lib/run-analyzer.mjs'
import {
  expectationEdges,
  judge,
  knownProblems,
  pairEdges,
  PORTED_RULES,
  strayFindings,
} from './lib/next-duplicates.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')
const KNOWN_PATH = join(ROOT, 'scripts', 'check', 'next-duplicates-known.json')
const TARGETS = ['fixtures/duplicates', 'fixtures/observed', 'fixtures/mixed']
const ENV = { VITEST_DEDUP_ENGINE: 'next' }

/** One whole-directory run per target, then the pairwise judgment over its fixture headers. */
export function runCli(argv) {
  const binary = argv[0] ?? defaultBinary()
  const known = JSON.parse(readFileSync(KNOWN_PATH, 'utf8')).cases
  const lines = []
  const failures = knownProblems(known).map((message) => ({ id: null, message: `FAIL ${message}` }))
  const held = []
  const consumed = new Set()
  let fixturesTotal = 0
  let requiredTotal = 0
  for (const target of TARGETS) {
    const { fixtureNames, knownFiles } = enumerateFixtures(target)
    if (fixtureNames.length === 0) {
      failures.push({
        id: null,
        message: `FAIL ${target}: the directory holds no test-named fixtures. A scanned directory that contributes no assertions is indistinguishable from a passing one`,
      })
      continue
    }
    const parsed = new Map()
    for (const name of fixtureNames) {
      const result = parseFixture(readFileSync(join(target, name), 'utf8'), name, knownFiles)
      if (result.error) {
        failures.push({ id: null, message: `FAIL ${join(target, name)}: ${result.error}` })
      } else {
        parsed.set(name, result)
      }
    }
    const { records } = runAnalyzerJsonl(binary, [target], ENV)
    const summary = records.find((record) => record.type === 'summary')
    if (!summary) throw new Error(`${target}: the run produced no summary record`)
    if (!(summary.testsAnalyzed > 0)) {
      failures.push({
        id: null,
        message: `FAIL ${target}: the next engine analyzed ${summary.testsAnalyzed} test(s); a run that measured nothing cannot pass`,
      })
    }
    for (const finding of strayFindings(records)) {
      failures.push({
        id: null,
        message: `FAIL ${target}: ${finding.rule} reported a finding under the next engine, which does not port it. Move the expectation through the corpus gate's engine axis; do not let the rule pass silently`,
      })
    }
    const linked = pairEdges(records)
    const { required, declared, forbidden } = expectationEdges(parsed)
    if (required.length === 0) {
      failures.push({
        id: null,
        message: `FAIL ${target}: no duplicate-rule expectation was asserted. A directory that judges nothing about the rules this gate exists to pin is a silent success`,
      })
    }
    const outcome = judge({ required, declared, forbidden, linked, known, scope: target })
    failures.push(...outcome.failures)
    held.push(...outcome.held)
    for (const id of outcome.consumed) consumed.add(id)
    fixturesTotal += fixtureNames.length
    requiredTotal += required.length
    lines.push(
      `${target}: ${fixtureNames.length} fixture(s), ${summary.testsAnalyzed} test(s) analyzed ` +
        `under next, ${linked.size} pair(s) linked by the ported rules`,
    )
  }
  for (const entry of known) {
    if (!consumed.has(entry.id)) {
      failures.push({
        id: null,
        message: `FAIL known entry "${entry.id}" no longer applies: its delta is gone or changed. Remove it -- ${entry.reason}`,
      })
    }
  }
  lines.unshift(
    `next duplicates: ${fixturesTotal} fixture(s) across ${TARGETS.length} directories, ` +
      `${requiredTotal} required pair(s), ${held.length} known delta(s) held`,
  )
  return {
    status: failures.length === 0 ? 0 : 1,
    lines: [...lines, ...held, ...failures.map((failure) => failure.message)],
  }
}

runAsGate(import.meta.url, runCli, 'next duplicates')
