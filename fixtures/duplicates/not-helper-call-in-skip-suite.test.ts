// Twin of helper-call-in-suite-twins.test.ts with the enclosing suite skipped: a call inside
// `describe.skip` would register inert cases through the expansion, and the engine does not
// model inherited skip on expanded cases, so the whole file's expansion refuses and every test
// keeps today's exclusion. Named weakening: drop `.skip` from the suite and the twin pair fires
// exactly as in the positive twin.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable the isolated variable is the enclosing suite's skip chain, which no participant fingerprint carries; the guard is the corpus gate's silence assertion plus the named weakening in the header
// @vitest-dedup:participant L26 "shared"
// @vitest-dedup:participant L22 "shared"
import { describe, test, expect } from 'vitest'

/** Registers one test per call, titled from the call's label. */
function makeTest(label: string) {
  test(`${label} works`, () => {
    const rows = 3
    expect(rows).toBe(3)
  })
}

describe.skip('s', () => {
  makeTest('shared')
})

describe('t', () => {
  makeTest('shared')
})
