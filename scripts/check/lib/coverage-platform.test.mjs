import assert from 'node:assert/strict'
import test from 'node:test'
import { coverageOnHost, missingTestTargets, targetReportingEnv } from './coverage-platform.mjs'

test('macOS enforces every floor; other hosts defer only native proof coverage to macOS CI', () => {
  const floors = { 'scripts/dev/revert-proof.mjs': 97, 'scripts/dev/proof-command.mjs': 95, 'scripts/dev/probe.mjs': 77, 'new-module.mjs': 100 }
  assert.deepEqual(coverageOnHost(floors, 'darwin'), { measured: floors, deferred: {} })
  for (const platform of ['linux', 'win32']) {
    assert.deepEqual(coverageOnHost(floors, platform), {
      measured: { 'scripts/dev/probe.mjs': 77, 'new-module.mjs': 100 },
      deferred: { 'scripts/dev/revert-proof.mjs': 97, 'scripts/dev/proof-command.mjs': 95 },
    })
  }
  assert.deepEqual(coverageOnHost(floors), coverageOnHost(floors, process.platform))
})

test('a test target is missing unless Cargo reported running it', () => {
  const log = [
    '     Running unittests src/lib.rs (target/debug/deps/vitest_dedup-1)',
    '     Running tests/cli_ux.rs (target/debug/deps/cli_ux-2)',
    '     Running tests\\windows_paths.rs (target\\debug\\deps\\windows_paths-3)',
    'test result: ok. 4 passed; 0 failed',
    'not a line: Running tests/quoted.rs',
    '\x1b[1m\x1b[92m     Running\x1b[0m tests/coloured.rs (target/debug/deps/coloured-4)',
  ].join('\n')
  assert.deepEqual(missingTestTargets(log, ['tests/cli_ux.rs', 'tests/windows_paths.rs', 'tests/coloured.rs', 'tests/quoted.rs', 'tests/never.rs']), ['tests/quoted.rs', 'tests/never.rs'])
  assert.deepEqual(missingTestTargets(log, []), [])
})

test('the target check runs Cargo with default status output, whatever quiet or verbose setting is inherited', () => {
  const inherited = { CARGO_TERM_QUIET: 'true', CARGO_TERM_VERBOSE: 'true', PATH: '/bin' }
  assert.deepEqual(targetReportingEnv(inherited), { CARGO_TERM_QUIET: 'false', CARGO_TERM_VERBOSE: 'false', PATH: '/bin' })
  assert.equal(inherited.CARGO_TERM_QUIET, 'true')
  assert.deepEqual(targetReportingEnv({}), { CARGO_TERM_QUIET: 'false', CARGO_TERM_VERBOSE: 'false' })
})
