import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { ALL_RULES } from './lib/fixture-directives.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const figures = (tests) => ({ tests, files: 1, reasons: { 'unresolved-context': { tests, files: 1 } } })

/** Give the owned project a pinned commit without depending on the checkout's history or hooks. */
function pinProject(path) {
  mkdirSync(path)
  const git = (args, input) => execFileSync('git', args, { cwd: path, input, encoding: 'utf8', timeout: 10_000, stdio: 'pipe' }).trim()
  git(['init', '--quiet'])
  const tree = git(['mktree'], '')
  const commit = git(['hash-object', '-t', 'commit', '-w', '--stdin'],
    `tree ${tree}\nauthor Fixture <fixture@example.com> 0 +0000\ncommitter Fixture <fixture@example.com> 0 +0000\n\ntest(corpus): seed owned project\n`)
  git(['update-ref', 'HEAD', commit])
  return commit
}

/** Run the real gate against isolated manifests and local Node programs emitting owned summaries. */
function runGate(specs, update = true) {
  const root = mkdtempSync(join(tmpdir(), 'external-corpus-update-'))
  try {
    cpSync(join(here, 'lib'), join(root, 'lib'), { recursive: true })
    cpSync(join(here, 'run-external-corpus.mjs'), join(root, 'run-external-corpus.mjs'))
    const clones = join(root, 'clones')
    mkdirSync(clones)
    const projects = specs.map((spec, index) => {
      const path = join(clones, `project-${index}`)
      const commit = pinProject(path)
      const summary = {
        type: 'summary', testsAnalyzed: 10, filesAnalyzed: 1, errors: 0, warnings: 0,
        unresolvedTests: 2, incomplete: true, incompleteCauses: { filesGlobalsUndetermined: 0 },
        exclusions: { tests: 2, files: 1, reasons: [{ reason: 'unresolved-context', tests: 2, files: 1 }] },
        ...spec.summary,
      }
      // Node accepts a directory with index.js as its program, avoiding platform-specific shebangs.
      writeFileSync(join(path, 'index.js'), `console.log(${JSON.stringify(JSON.stringify(summary))});\n`)
      return {
        repo: `owned/project-${index}`, commit, expectedGlobalsDetection: 'disabled', maxGlobalsUndeterminedFiles: 0,
        knownTruePositives: [], ...spec.project,
        baseline: { tests: 10, files: 1, errors: 0, warnings: 0, unresolved: 1, findingIds: [], excluded: figures(1), ...spec.baseline },
      }
    })
    const rulesWithPositives = projects.some((project) => project.knownTruePositives.length > 0) ? ['duplicate-test'] : []
    const manifest = {
      projects,
      aggregate: { tests: projects.length * 10, unresolved: projects.reduce((sum, project) => sum + project.baseline.unresolved, 0) },
      retentionAbsences: Object.fromEntries([...ALL_RULES].filter((rule) => !rulesWithPositives.includes(rule)).map((rule) =>
        [rule, 'Scanned the owned summary-only fixture; re-take when it emits findings.'])),
    }
    const manifestPath = join(root, 'corpus-projects.json')
    writeFileSync(manifestPath, JSON.stringify(manifest))
    const before = JSON.parse(readFileSync(manifestPath, 'utf8'))
    const run = spawnSync(process.execPath, [join(root, 'run-external-corpus.mjs'), '--clone-dir', clones, '--binary', process.execPath, ...(update ? ['--update'] : [])], {
      cwd: root, encoding: 'utf8', timeout: 15_000, maxBuffer: 1 << 20,
    })
    assert.ifError(run.error)
    assert.equal(run.signal, null)
    assert.match(run.stdout, /excluded from comparison: 2 of 10 tests|10 tests, 0E 0W, 2 unresolved/)
    return { before, after: JSON.parse(readFileSync(manifestPath, 'utf8')), status: run.status, output: run.stdout + run.stderr }
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

const failures = [
  ['wrong globals mode', { project: { expectedGlobalsDetection: 'enabled' } }],
  ['missing globals ceiling', { project: { maxGlobalsUndeterminedFiles: undefined } }],
  ['globals attribution worsened', { summary: { incompleteCauses: { filesGlobalsUndetermined: 1 } } }],
  ['globals ceiling needs tightening', { project: { maxGlobalsUndeterminedFiles: 1 } }],
  ['pending globals expectation now holds', { project: { globalsDetectionPending: 'previously unresolved' } }],
]

for (const [name, spec] of failures) {
  test(`--update preserves the entire baseline after ${name}, even with count and exclusion drift`, () => {
    const { before, after, status, output } = runGate([spec])
    assert.equal(status, 1)
    assert.match(output, /FAIL (?:manifest: )?owned\/project-0/)
    assert.deepEqual(after.projects[0].baseline, before.projects[0].baseline)
    assert.deepEqual(after.aggregate, before.aggregate)
  })
}

test('--update does not initialize missing identities or exclusions after a globals failure', () => {
  const { before, after, status } = runGate([{
    project: { expectedGlobalsDetection: 'enabled' },
    baseline: { unresolved: 2, findingIds: undefined, excluded: undefined },
  }])
  assert.equal(status, 1)
  assert.deepEqual(after.projects[0].baseline, before.projects[0].baseline)
})

test('--update still records valid drift and recomputes the aggregate', () => {
  const { after, status, output } = runGate([{}])
  assert.equal(status, 1, 'the current run still reports the drift it measured')
  assert.doesNotMatch(output, /FAIL /)
  assert.deepEqual(after.projects[0].baseline, { tests: 10, files: 1, errors: 0, warnings: 0, unresolved: 2, findingIds: [], excluded: figures(2) })
  assert.deepEqual(after.aggregate, { tests: 10, unresolved: 2 })
})

test('--update initializes missing figures on a valid unchanged run', () => {
  const { after, status } = runGate([{ baseline: { unresolved: 2, findingIds: undefined, excluded: undefined } }])
  assert.equal(status, 0)
  assert.deepEqual(after.projects[0].baseline.excluded, figures(2))
  assert.deepEqual(after.projects[0].baseline.findingIds, [])
})

test('a held pending globals expectation still permits a reviewed update', () => {
  const { after, output } = runGate([{ project: { expectedGlobalsDetection: 'enabled', globalsDetectionPending: 'owned known limitation' } }])
  assert.match(output, /globals detection still/)
  assert.doesNotMatch(output, /FAIL /)
  assert.deepEqual(after.projects[0].baseline.excluded, figures(2))
})

test('one failed project does not prevent another project from recording valid drift', () => {
  const { before, after, status } = runGate([{ project: { expectedGlobalsDetection: 'enabled' } }, {}])
  assert.equal(status, 1)
  assert.deepEqual(after.projects[0].baseline, before.projects[0].baseline)
  assert.deepEqual(after.projects[1].baseline.excluded, figures(2))
  assert.deepEqual(after.aggregate, { tests: 20, unresolved: 3 })
})

test('lost known positives still prevent baseline updates', () => {
  const { before, after, status, output } = runGate([{ project: { knownTruePositives: ['case.test.ts:1 (duplicate-test, redundant)'] } }])
  assert.equal(status, 1)
  assert.match(output, /KNOWN TRUE POSITIVE NO LONGER REPORTED/)
  assert.deepEqual(after.projects[0].baseline, before.projects[0].baseline)
})

test('a run without --update never changes its manifest', () => {
  const { before, after, status } = runGate([{}], false)
  assert.equal(status, 1)
  assert.deepEqual(after, before)
})
