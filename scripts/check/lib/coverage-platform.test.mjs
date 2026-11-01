import assert from 'node:assert/strict'
import test from 'node:test'
import { coverageOnHost } from './coverage-platform.mjs'

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
