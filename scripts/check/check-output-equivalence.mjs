// Output equivalence against a released baseline.
//
// Refactors and performance work must not change what the analyzer reports. This check builds the
// analyzer at a baseline Git ref and runs the corpus, recall and isolation gates and the Rust test
// executables with a shim in place of the binary. Every invocation runs both binaries on the same
// input and compares exit status, stdout, stderr and every file the run writes, after removing the
// terminal detection time, the process peak RSS and the version string. Any difference fails the
// check, and so does a suite that made no invocation: a gate that silently stopped calling the
// binary would otherwise pass vacuously.
//
// The gates still assert their own oracles against the candidate. A gate failure is reported
// separately from equivalence, so a run can show "behavior unchanged, oracle still failing" apart
// from "behavior changed".
//
// Usage: node scripts/check/check-output-equivalence.mjs [--ref REF] [--suites a,b] [CANDIDATE]
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { runAsGate } from './lib/gate-main.mjs'
import { readRecords, summarize } from './lib/output-equivalence.mjs'
import { defaultBinary } from './lib/run-analyzer.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, '..', '..')

/** Environment variable `assert_cmd`'s `Command::cargo_bin` reads at run time, so tests can be redirected. */
const BINARY_VARIABLE = 'CARGO_BIN_EXE_vitest-dedup'

/**
 * The suites, in run order. Each names the commands it runs; `{shim}` stands for the shim binary.
 * `rust` has no fixed command: it is every test executable Cargo builds, found when it runs.
 */
export const SUITES = Object.freeze({
  corpus: [['node', ['scripts/check/check-duplicates-corpus.mjs', '{shim}']]],
  recall: [['node', ['scripts/check/check-recall.mjs', '{shim}']]],
  isolation: [['node', ['scripts/check/check-fixture-isolation.mjs', '{shim}']]],
  rust: null,
})

/**
 * Parses `[--ref REF] [--suites a,b] [CANDIDATE]`.
 *
 * @param {string[]} argv Arguments after the script name.
 * @returns {{ref: string|null, suites: string[], candidate: string}}
 */
export function parseArguments(argv) {
  const options = { ref: null, suites: Object.keys(SUITES), candidate: defaultBinary() }
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--ref' || argument === '--suites') {
      const value = argv[index + 1]
      assert.ok(value !== undefined && !value.startsWith('--'), `${argument} needs a value`)
      index += 1
      if (argument === '--ref') options.ref = value
      else options.suites = value.split(',')
    } else {
      assert.ok(!argument.startsWith('--'), `unknown option ${argument}`)
      options.candidate = argument
    }
  }
  for (const suite of options.suites) {
    assert.ok(suite in SUITES, `unknown suite ${suite}; known: ${Object.keys(SUITES).join(', ')}`)
  }
  assert.ok(new Set(options.suites).size === options.suites.length, '--suites names a suite twice')
  return options
}

/** Output of a command that must succeed. */
function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, ...options })
  assert.equal(result.status, 0, `${command} ${args.join(' ')}: ${result.error?.message ?? result.stderr}`)
  return result.stdout
}

/**
 * The newest release tag reachable from HEAD: the behavior every change is held to, so it follows
 * each release instead of going stale like a pinned one. `git describe` would pick the nearest tag,
 * not the highest version.
 */
export function newestReleaseTag(listed) {
  const tag = listed.split('\n')[0].trim()
  assert.ok(
    tag,
    'no release tag is reachable from HEAD; pass --ref REF, or fetch tags with `git fetch --tags --unshallow`',
  )
  return tag
}

/** Release binary built from `revision`, cached under `target/equivalence` by commit. */
function baselineBinary(revision) {
  const commit = run('git', ['rev-parse', '--verify', `${revision}^{commit}`], { cwd: root }).trim()
  const cache = join(root, 'target', 'equivalence')
  const binary = join(cache, commit, 'vitest-dedup')
  if (existsSync(binary)) return binary
  // Source and Cargo output are private to this process, so concurrent runs, for the same or
  // different refs, never delete or copy each other's build.
  const build = join(cache, commit, `build-${process.pid}`)
  const source = join(build, 'src')
  const target = join(build, 'target')
  try {
    mkdirSync(source, { recursive: true })
    const archive = spawnSync('git', ['archive', commit], { cwd: root, maxBuffer: 1024 * 1024 * 1024 })
    assert.equal(archive.status, 0, `git archive ${commit}: ${archive.stderr}`)
    run('tar', ['-x', '-C', source], { input: archive.stdout })
    process.stderr.write(`building baseline ${revision} (${commit.slice(0, 12)})\n`)
    run(
      'cargo',
      ['build', '--release', '--locked', '--manifest-path', join(source, 'Cargo.toml'), '--target-dir', target],
      {
        stdio: ['ignore', 'inherit', 'inherit'],
      },
    )
    // Stage under a private name and rename into place, so a reader never sees a partial binary.
    const staged = join(build, 'vitest-dedup')
    copyFileSync(join(target, 'release', 'vitest-dedup'), staged)
    chmodSync(staged, 0o755)
    renameSync(staged, binary)
  } finally {
    rmSync(build, { recursive: true, force: true })
  }
  return binary
}

/** Every test executable of the checked-out tree (library, binary and integration tests), built by Cargo when stale. */
function testExecutables() {
  const messages = run('cargo', ['test', '--locked', '--no-run', '--message-format', 'json-render-diagnostics'], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'inherit'],
  })
  const executables = messages
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .filter((message) => message.reason === 'compiler-artifact' && message.profile?.test && message.executable)
    .map((message) => message.executable)
  assert.ok(executables.length > 0, 'cargo built no test executable')
  return executables
}

/** The `[command, args]` pairs a suite runs, with `{shim}` filled in. */
function commandsFor(suite, shim) {
  if (suite === 'rust') return testExecutables().map((executable) => [executable, []])
  return SUITES[suite].map(([command, args]) => [command, args.map((arg) => arg.replace('{shim}', shim))])
}

const shimScript = join(here, 'lib', 'equivalence-shim.mjs')

/** Runs the check; returns the process exit status. */
function main(argv) {
  assert.notEqual(process.platform, 'win32', 'check:equivalence runs on macOS and Linux only')
  const options = parseArguments(argv)
  const candidate = resolve(options.candidate)
  assert.ok(existsSync(candidate), `${candidate} is missing; run \`cargo build --release --locked\``)
  const ref =
    options.ref ??
    newestReleaseTag(run('git', ['tag', '--merged', 'HEAD', '--list', 'v[0-9]*', '--sort=-v:refname'], { cwd: root }))
  const baseline = baselineBinary(ref)
  /** Version number printed by `binary --version`. */
  const version = (binary) => run(binary, ['--version']).trim().split(/\s+/).pop()
  const versions = [[version(candidate), version(baseline)]].filter(([from, to]) => from !== to)
  const scratch = mkdtempSync(join(tmpdir(), 'vitest-dedup-equivalence-'))
  try {
    const shim = join(scratch, 'vitest-dedup')
    // POSIX single quotes: nothing inside them expands, whatever the checkout path contains.
    writeFileSync(shim, `#!/bin/sh\nexec node '${shimScript.replaceAll("'", "'\\''")}' "$@"\n`)
    chmodSync(shim, 0o755)
    // One record file per shim invocation: the Rust tests run many shims at once.
    const log = join(scratch, 'equivalence-records')
    mkdirSync(log)
    const gateFailures = []
    for (const suite of options.suites) {
      process.stderr.write(`\n== ${suite}\n`)
      for (const [command, args] of commandsFor(suite, shim)) {
        const result = spawnSync(command, args, {
          cwd: root,
          stdio: ['ignore', 'pipe', 'inherit'],
          encoding: 'utf8',
          maxBuffer: 64 * 1024 * 1024,
          env: {
            ...process.env,
            [BINARY_VARIABLE]: shim,
            VITEST_DEDUP_EQUIVALENCE_BASELINE: baseline,
            VITEST_DEDUP_EQUIVALENCE_CANDIDATE: candidate,
            VITEST_DEDUP_EQUIVALENCE_LOG: log,
            VITEST_DEDUP_EQUIVALENCE_SUITE: suite,
            VITEST_DEDUP_EQUIVALENCE_VERSIONS: JSON.stringify(versions),
          },
        })
        if (result.status !== 0) {
          // Test harnesses report failures on stdout; keep its tail so a gate failure is diagnosable.
          process.stderr.write(`${(result.stdout ?? '').split('\n').slice(-40).join('\n')}\n`)
          gateFailures.push(`${suite}: ${command} (exit ${result.status ?? result.signal})`)
        }
      }
    }
    const records = readRecords(log)
    const verdict = summarize(records, options.suites)
    console.log(`\nbaseline ${ref}; candidate ${candidate}`)
    for (const [suite, count] of Object.entries(verdict.invocations)) console.log(`${suite}: ${count} invocations`)
    for (const record of verdict.mismatches.slice(0, 20)) {
      console.log(`\nMISMATCH [${record.suite}] ${record.cwd} ${record.args.join(' ')}`)
      for (const difference of record.differences) console.log(`  ${JSON.stringify(difference)}`)
    }
    if (verdict.mismatches.length > 20) console.log(`\n... ${verdict.mismatches.length - 20} more mismatches`)
    for (const suite of verdict.empty) console.log(`EMPTY ${suite}: it made no analyzer invocation`)
    for (const failure of gateFailures) console.log(`GATE FAILED ${failure}`)
    console.log(
      verdict.ok
        ? `\noutput equivalent: ${records.length} invocations, 0 mismatches`
        : `\noutput differs: ${verdict.mismatches.length} of ${records.length} invocations`,
    )
    return verdict.ok && gateFailures.length === 0 ? 0 : 1
  } finally {
    rmSync(scratch, { recursive: true, force: true })
  }
}

runAsGate(import.meta.url, (argv) => ({ status: main(argv), lines: [] }), 'output equivalence')
