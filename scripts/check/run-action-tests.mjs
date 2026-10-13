#!/usr/bin/env node
// Runs the GitHub Action's test suite and fails if it did not actually run tests.
//
// The floor is the point of this wrapper. CI previously invoked
// `node --test .github/actions/vitest-dedup/`, and Node resolved that directory to its `index.js`
// entry point, executed it as a single module, and reported "pass 1" — a green check over ZERO of
// the 49 real tests, for months of commits. A suite that silently runs nothing is worse than one
// that fails, because it is indistinguishable from success.
//
// Same failure class the fixture corpus gate guards against, and the reason that gate asserts an
// extraction count rather than only "no findings".
import { execFileSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'

const FILE = '.github/actions/vitest-dedup/index.test.mjs'
const MIN_TESTS = 40 // 49 today; a floor that catches wholesale loss, not every added test.

let output
let failed = false
try {
  output = execFileSync('node', ['--test', FILE], { encoding: 'utf8' })
} catch (error) {
  if (error.stdout === undefined) throw error
  output = error.stdout
  failed = true
}

// Negative Action tests intentionally print ::error:: and ::warning:: messages. When this
// wrapper replays their captured stdout in Actions, those strings would become annotations on a
// green CI job. Pause workflow-command processing only around the captured test output; the
// wrapper's own failure diagnostics below must still be visible to the runner.
const stopToken = randomUUID()
process.stdout.write(`::stop-commands::${stopToken}\n`)
try {
  process.stdout.write(output)
  if (!output.endsWith('\n')) process.stdout.write('\n')
} finally {
  process.stdout.write(`::${stopToken}::\n`)
}

const read = (label) => {
  const match = output.match(new RegExp(`^. ${label} (\\d+)$`, 'm'))
  return match ? Number(match[1]) : null
}
const pass = read('pass')
const fail = read('fail')

if (pass === null || fail === null) {
  console.error('FAIL: could not read a test summary from the runner output.')
  process.exit(1)
}
if (fail > 0 || failed) {
  console.error(`FAIL: ${fail} action test(s) failed.`)
  process.exit(1)
}
if (pass < MIN_TESTS) {
  console.error(
    `FAIL: only ${pass} action tests ran, expected at least ${MIN_TESTS}. ` +
      'A suite that runs almost nothing still reports success, which is why this floor exists.',
  )
  process.exit(1)
}
console.log(`ok: ${pass} action tests ran and passed.`)
