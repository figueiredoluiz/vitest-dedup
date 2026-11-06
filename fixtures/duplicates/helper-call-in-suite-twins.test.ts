// A helper called inside two twin suites registers its template into each: the expanded cases
// take the caller's suite path (`a > shared works`, `b > shared works`) and pair exactly as two
// hand-written twin suites do -- identical bodies under equivalent contexts pair regardless of
// the suite name, so the claim is duplicate-test. One call per suite also pins that each
// invocation owns its cases: two invocations of one template with equal arguments are two
// runtime cases, not one.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L15 "a > shared works"
// @vitest-dedup:participant L15 "b > shared works"
import { describe, test, expect } from 'vitest'

/** Registers one test per call, titled from the call's label. */
function makeTest(label: string) {
  test(`${label} works`, () => {
    const rows = 3
    expect(rows).toBe(3)
  })
}

describe('a', () => {
  makeTest('shared')
})

describe('b', () => {
  makeTest('shared')
})
