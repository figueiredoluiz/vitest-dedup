#!/usr/bin/env node
// Runs the Rust tests of a changed area for `verify:quick`, and fails when none ran: a filter that
// matches nothing makes `cargo test` print "0 passed" and exit 0, which reads as a pass.
//
//   cargo-tests.mjs lib [module...]   library unit tests under each `module::`; none named, all of them
//   cargo-tests.mjs bin               builds the binary first: `cargo test --bin` does not rebuild the
//                                     standalone binary the assert_cmd tests run
//   cargo-tests.mjs test <target...> [-- <libtest args>]  named integration targets and optional filter
import { spawnSync } from 'node:child_process'
import { existsSync, realpathSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const MAX_BUFFER = 256 * 1024 * 1024

/**
 * Sum the tests libtest reports as run across every test binary in `output`.
 * @param {string} output Captured `cargo test` stdout.
 * @returns {number} Passed plus failed tests; ignored and filtered-out ones did not run.
 */
export function testsRun(output) {
  let total = 0
  for (const [, passed, failed] of output.matchAll(/^test result: \w+\. (\d+) passed; (\d+) failed;/gm)) total += Number(passed) + Number(failed)
  return total
}

/**
 * Keep the modules that have unit tests of their own, from `cargo test -- --list`.
 * @param {string} listing Captured listing, one `path::name: test` line per test.
 * @param {string[]} modules Top-level library module names.
 * @returns {{covered: string[], uncovered: string[]}} Modules with and without a test under them.
 */
export function moduleCoverage(listing, modules) {
  const names = listing.split(/\r?\n/).flatMap(line => /^(.+): test$/.exec(line)?.[1] ?? [])
  const covered = modules.filter(module => names.some(name => name.startsWith(`${module}::`)))
  return { covered, uncovered: modules.filter(module => !covered.includes(module)) }
}

/**
 * Plan the Cargo invocations for one request, given the library listing when it is needed.
 * @param {string[]} args `lib [module...]`, `bin` or `test <target...> [-- <libtest args>]`.
 * @param {() => string} list Reads the library's test listing.
 * @returns {{runs: string[][], notes: string[]}} Cargo argument lists in order, and what was widened.
 * @throws {Error} On an unknown kind or a `test` request without targets.
 */
export function plan(args, list) {
  const [kind, ...names] = args
  const locked = ['--locked']
  if (kind === 'bin') return { runs: [['build', ...locked], ['test', '--bin', 'vitest-dedup', ...locked]], notes: [] }
  if (kind === 'test') {
    const separator = names.indexOf('--')
    const targets = separator < 0 ? names : names.slice(0, separator)
    if (!targets.length) throw new Error('cargo-tests.mjs test needs at least one target')
    const filter = separator < 0 ? [] : names.slice(separator)
    return { runs: [['test', ...locked, ...targets.flatMap(name => ['--test', name]), ...filter]], notes: [] }
  }
  if (kind !== 'lib') throw new Error('Usage: cargo-tests.mjs lib [module...] | bin | test <target...>')
  if (!names.length) return { runs: [['test', '--lib', ...locked]], notes: [] }
  const { uncovered } = moduleCoverage(list(), names)
  // A module without unit tests of its own is covered from elsewhere in the library; which tests
  // those are is not recorded, so the whole library suite runs.
  if (uncovered.length) return { runs: [['test', '--lib', ...locked]], notes: [`No unit tests under ${uncovered.map(module => `${module}::`).join(', ')}; running the whole library suite.`] }
  return { runs: [['test', '--lib', ...locked, '--', ...names.map(module => `${module}::`)]], notes: [] }
}

/**
 * Run the planned invocations, echoing their output, and fail when no test ran.
 * @param {string[]} args Command-line arguments after the script name.
 * @param {object} [options] Spawn and stream adapters, for tests.
 * @returns {number} Exit status.
 */
export function main(args, { spawn = spawnSync, out = process.stdout, err = process.stderr } = {}) {
  /** Run Cargo, echo its output, and return the result. */
  const cargo = cargoArgs => {
    const result = spawn('cargo', cargoArgs, { encoding: 'utf8', maxBuffer: MAX_BUFFER, stdio: ['ignore', 'pipe', 'pipe'] })
    out.write(result.stdout ?? '')
    err.write(result.stderr ?? '')
    return result
  }
  const { runs, notes } = plan(args, () => {
    const listed = cargo(['test', '--lib', '--locked', '--', '--list'])
    if (listed.error || listed.status !== 0) throw new Error(`Listing the library tests failed${listed.error ? `: ${listed.error.message}` : ` (exit ${listed.status ?? listed.signal})`}`)
    return listed.stdout
  })
  for (const note of notes) out.write(`${note}\n`)
  let total = 0
  for (const cargoArgs of runs) {
    const result = cargo(cargoArgs)
    if (result.error || result.status !== 0) {
      err.write(`cargo ${cargoArgs.join(' ')} failed${result.error ? `: ${result.error.message}` : ` (exit ${result.status ?? result.signal})`}\n`)
      return typeof result.status === 'number' && result.status !== 0 ? result.status : 1
    }
    total += testsRun(result.stdout)
  }
  if (total === 0) {
    err.write(`No test ran for: ${args.join(' ')}\n`)
    return 1
  }
  out.write(`${total} tests ran.\n`)
  return 0
}

if (process.argv[1] && existsSync(process.argv[1]) && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  try { process.exitCode = main(process.argv.slice(2)) }
  catch (error) { console.error(error.message); process.exitCode = 1 }
}
