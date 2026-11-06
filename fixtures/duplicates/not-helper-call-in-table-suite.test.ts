// Twin of helper-call-in-suite-twins.test.ts with the enclosing suite a table (`describe.each`):
// a helper call inside one registers once per generated suite, and that population is other
// machinery (the table rows proof), so the file's expansion refuses. Named weakening: replace
// `describe.each([1])` with `describe` and the twin pair fires exactly as in the positive twin.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable the isolated variable is the enclosing suite's table chain, which no participant fingerprint carries; the guard is the corpus gate's silence assertion plus the named weakening in the header
// @vitest-dedup:participant L21 "shared"
// @vitest-dedup:participant L25 "shared"
import { describe, test, expect } from 'vitest'

/** Registers one test per call, titled from the call's label. */
function makeTest(label: string) {
  test(`${label} works`, () => {
    const rows = 3
    expect(rows).toBe(3)
  })
}

describe.each([1])('s %s', () => {
  makeTest('shared')
})

describe('t', () => {
  makeTest('shared')
})
