// Tests for the shared analyzer invocation library. `runAnalyzerJsonl` spawns a real binary and
// is covered by the gates that use it; the default binary path is pure and is pinned here because
// a wrong spelling fails every gate's existence check on one platform only (Windows), where no
// gate runs in CI.
import assert from 'node:assert/strict'
import { chmodSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { defaultBinary, isCompletedAnalysisExit, runAnalyzerJsonl } from './run-analyzer.mjs'

test('a run that never finishes is stopped at its time limit and reported as such', { skip: process.platform === 'win32' }, () => {
  const binary = join(mkdtempSync(join(tmpdir(), 'run-analyzer-')), 'never-finishes')
  writeFileSync(binary, '#!/bin/sh\nexec sleep 60\n')
  chmodSync(binary, 0o755)
  const started = Date.now()
  assert.throws(() => runAnalyzerJsonl(binary, [], {}, { timeoutMs: 200 }), /did not finish within 0\.2 s and was stopped/)
  assert.ok(Date.now() - started < 10_000, 'the limit stops the run, not the sleep')
})

test('the default binary carries .exe on Windows and nothing elsewhere', () => {
  assert.equal(defaultBinary({ platform: 'win32' }), './target/release/vitest-dedup.exe')
  assert.equal(defaultBinary({ platform: 'linux' }), './target/release/vitest-dedup')
  assert.equal(defaultBinary({ platform: 'darwin' }), './target/release/vitest-dedup')
})

test('an absolute root keeps the same name under its target directory', () => {
  assert.equal(defaultBinary({ platform: 'win32', root: '/repo' }), join('/repo', 'target', 'release', 'vitest-dedup.exe'))
  assert.equal(defaultBinary({ platform: 'linux', root: '/repo' }), join('/repo', 'target', 'release', 'vitest-dedup'))
})

test('the running platform is the default, so the gates need no argument', () => {
  assert.equal(defaultBinary(), defaultBinary({ platform: process.platform }))
})

test('only clean and findings exits carry a completed quality result', () => {
  assert.equal(isCompletedAnalysisExit(0), true)
  assert.equal(isCompletedAnalysisExit(1), true)
  assert.equal(isCompletedAnalysisExit(2), false)
  assert.equal(isCompletedAnalysisExit(null), false)
})
