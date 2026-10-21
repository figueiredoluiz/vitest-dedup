import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { chmodSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'
import { SUITES, newestReleaseTag, parseArguments } from './check-output-equivalence.mjs'
import { readRecords } from './lib/output-equivalence.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const shimPath = join(here, 'lib', 'equivalence-shim.mjs')
/** The stand-in binaries are shebang scripts, which Windows cannot execute; the check itself is POSIX only. */
const POSIX_ONLY = process.platform === 'win32'

test('arguments default to every suite and the release binary', () => {
  const options = parseArguments([])
  assert.deepEqual(options.suites, Object.keys(SUITES))
  assert.equal(options.ref, null)
  assert.match(options.candidate, /target\/release\/vitest-dedup/)
})

test('arguments select a ref, suites and a candidate', () => {
  const options = parseArguments(['--ref', 'v0.5.1', '--suites', 'rust,corpus', './b'])
  assert.deepEqual(options, { ref: 'v0.5.1', suites: ['rust', 'corpus'], candidate: './b' })
})

test('arguments reject what they cannot honour', () => {
  assert.throws(() => parseArguments(['--suites', 'nope']), /unknown suite nope/)
  assert.throws(() => parseArguments(['--suites', 'corpus,corpus']), /names a suite twice/)
  assert.throws(() => parseArguments(['--ref']), /--ref needs a value/)
  assert.throws(() => parseArguments(['--ref', '--suites', 'corpus']), /--ref needs a value/)
  assert.throws(() => parseArguments(['--suites']), /--suites needs a value/)
  assert.throws(() => parseArguments(['--fast']), /unknown option --fast/)
})

test('every scripted suite names a gate that exists and receives the shim', () => {
  for (const [suite, commands] of Object.entries(SUITES)) {
    if (commands === null) continue
    assert.ok(commands.length > 0, `${suite} runs nothing`)
    for (const [command, args] of commands) {
      assert.equal(command, 'node')
      assert.ok(existsSync(resolve(here, '..', '..', args[0])), `${suite}: ${args[0]} does not exist`)
      assert.ok(args.includes('{shim}'), `${suite} would run the gate against its default binary`)
    }
  }
})

test('the newest listed tag is chosen and an empty list is refused', { skip: POSIX_ONLY }, () => {
  assert.equal(newestReleaseTag('v0.6.0\nv0.5.1\nv0.5.0\n'), 'v0.6.0')
  assert.throws(() => newestReleaseTag(''), /no release tag is reachable/)
  assert.throws(() => newestReleaseTag('\n'), /no release tag is reachable/)
})

/**
 * Runs the shim over two stand-in binaries written as Node scripts and returns the record it kept.
 *
 * @param {{baseline: string, candidate: string, args?: string[], versions?: string[][]}} spec
 *   Each side is the body of a script that sees `args` (the arguments) and `cwd`.
 */
function shimmed({ baseline, candidate, args = ['.'], versions = [], prepare = () => {}, breakBaseline = false }) {
  const root = mkdtempSync(join(tmpdir(), 'shim-test-'))
  try {
    const write = (name, body) => {
      const path = join(root, name)
      writeFileSync(
        path,
        `#!/usr/bin/env node\nconst fs = require('node:fs')\nconst args = process.argv.slice(2)\n${body}\n`,
      )
      chmodSync(path, 0o755)
      return path
    }
    const project = join(root, 'project')
    mkdirSync(project)
    prepare(project)
    const log = join(root, 'log')
    mkdirSync(log)
    const run = spawnSync(process.execPath, [shimPath, ...args], {
      cwd: project,
      encoding: 'utf8',
      env: {
        ...process.env,
        VITEST_DEDUP_EQUIVALENCE_BASELINE: breakBaseline ? join(root, 'missing-binary') : write('baseline', baseline),
        VITEST_DEDUP_EQUIVALENCE_CANDIDATE: write('candidate', candidate),
        VITEST_DEDUP_EQUIVALENCE_LOG: log,
        VITEST_DEDUP_EQUIVALENCE_SUITE: 'unit',
        VITEST_DEDUP_EQUIVALENCE_VERSIONS: JSON.stringify(versions),
      },
    })
    const [record] = readRecords(log)
    const parts = record.differences.map((difference) => difference.part)
    return { run, record, parts, project, root }
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

const SAME = "console.log('VitestDedup v0.6.0\\nDetection time: ' + (process.env.T ?? '5') + ' ms')"

test("the shim reproduces the candidate's output and exit status", { skip: POSIX_ONLY }, () => {
  const { run, parts } = shimmed({
    baseline: "console.log('from baseline'); console.error('baseline stderr'); process.exitCode = 3",
    candidate: "console.log('from candidate'); console.error('candidate stderr'); process.exitCode = 1",
  })
  assert.equal(run.stdout, 'from candidate\n')
  assert.equal(run.stderr, 'candidate stderr\n')
  assert.equal(run.status, 1)
  assert.deepEqual(parts, ['exit', 'stdout', 'stderr'])
})

test('equal output with different timing and a different version is equivalent', { skip: POSIX_ONLY }, () => {
  const baseline = "console.log('VitestDedup v0.6.0'); console.log('Detection time: 1.5 ms')"
  const candidate = "console.log('VitestDedup v0.7.0'); console.log('Detection time: 80 ms')"
  const { record } = shimmed({ baseline, candidate, versions: [['0.7.0', '0.6.0']] })
  assert.deepEqual(record.differences, [])
})

test('a version change outside the declared pair is a difference', { skip: POSIX_ONLY }, () => {
  const baseline = "console.log('VitestDedup v0.6.0')"
  const candidate = "console.log('VitestDedup v0.7.0')"
  assert.deepEqual(shimmed({ baseline, candidate }).parts, ['stdout'])
  assert.deepEqual(shimmed({ baseline, candidate, versions: [['0.8.0', '0.6.0']] }).parts, ['stdout'])
})

test('stdout, stderr and exit differences are each reported', { skip: POSIX_ONLY }, () => {
  assert.deepEqual(shimmed({ baseline: "console.log('a')", candidate: "console.log('b')" }).parts, ['stdout'])
  assert.deepEqual(shimmed({ baseline: "console.error('a')", candidate: "console.error('b')" }).parts, ['stderr'])
  assert.deepEqual(shimmed({ baseline: '', candidate: 'process.exitCode = 2' }).parts, ['exit'])
})

const WRITE_REPORT = (text) =>
  `fs.mkdirSync('reports', { recursive: true }); fs.writeFileSync('reports/vitest-dedup.json', ${JSON.stringify(text)})`

test('a report that differs between the binaries is reported by file', { skip: POSIX_ONLY }, () => {
  const { parts } = shimmed({
    baseline: WRITE_REPORT('{"a":1}'),
    candidate: WRITE_REPORT('{"a":2}'),
    args: ['.', '--reporters', 'json'],
  })
  assert.equal(parts.length, 1)
  assert.match(parts[0], /^file:.*reports\/vitest-dedup\.json$/)
})

test('a report only one binary writes is a difference', { skip: POSIX_ONLY }, () => {
  const { parts } = shimmed({ baseline: '', candidate: WRITE_REPORT('{}'), args: ['.', '--reporters', 'json'] })
  assert.equal(parts.length, 1)
  assert.match(parts[0], /^file:.*reports\/vitest-dedup\.json$/)
})

test('both binaries start from the same state: the baseline run is undone first', { skip: POSIX_ONLY }, () => {
  // The candidate fails if the baseline's report is still there when it starts.
  const candidate = `if (fs.existsSync('reports/vitest-dedup.json')) process.exit(9); ${WRITE_REPORT('{}')}`
  const { run, parts } = shimmed({ baseline: WRITE_REPORT('{}'), candidate, args: ['.', '--reporters', 'json'] })
  assert.equal(run.status, 0)
  assert.deepEqual(parts, [])
})

test('a report the arguments do not name is not watched', { skip: POSIX_ONLY }, () => {
  const { parts } = shimmed({
    baseline: WRITE_REPORT('{"a":1}'),
    candidate: WRITE_REPORT('{"a":2}'),
    args: ['.', '--reporters', 'terminal'],
  })
  assert.deepEqual(parts, [])
})

test('an updated baseline file is compared', { skip: POSIX_ONLY }, () => {
  const write = (text) => `fs.writeFileSync('b.json', ${JSON.stringify(text)})`
  const args = ['.', '--baseline', 'b.json', '--update-baseline']
  assert.deepEqual(shimmed({ baseline: write('1'), candidate: write('1'), args }).parts, [])
  assert.deepEqual(shimmed({ baseline: write('1'), candidate: write('2'), args }).parts.length, 1)
})

test('--output moves the compared report', { skip: POSIX_ONLY }, () => {
  const write = (text) => `fs.writeFileSync('r.json', ${JSON.stringify(text)})`
  const args = ['.', '--reporters', 'json', '--output', 'r']
  const { parts } = shimmed({ baseline: write('1'), candidate: write('2'), args })
  assert.equal(parts.length, 1)
  assert.match(parts[0], /^file:.*\/r\.json$/)
})

test('the shim refuses to run outside the equivalence check', { skip: POSIX_ONLY }, () => {
  const run = spawnSync(process.execPath, [shimPath, '.'], {
    encoding: 'utf8',
    env: Object.fromEntries(
      Object.entries(process.env).filter(([name]) => !name.startsWith('VITEST_DEDUP_EQUIVALENCE')),
    ),
  })
  assert.equal(run.status, 2)
  assert.match(run.stderr, /run it through scripts\/check\/check-output-equivalence\.mjs/)
})

test('a refused write location is recorded as a difference, not passed through', { skip: POSIX_ONLY }, () => {
  const { run, record, parts } = shimmed({
    baseline: 'process.exitCode = 1',
    candidate: 'process.exitCode = 1',
    args: ['.', '--reporters', 'json', '--output', 'a/../b'],
  })
  assert.equal(run.status, 70)
  assert.deepEqual(parts, ['shim'])
  assert.match(record.differences[0].error, /unsupported write location/)
  assert.match(run.stderr, /equivalence shim: unsupported write location/)
})

test('a binary that cannot be started is recorded as a difference', { skip: POSIX_ONLY }, () => {
  const { run, parts } = shimmed({ baseline: '', candidate: '', args: ['.'], breakBaseline: true })
  assert.equal(run.status, 70)
  assert.deepEqual(parts, ['shim'])
})
