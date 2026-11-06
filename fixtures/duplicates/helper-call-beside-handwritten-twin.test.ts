// A helper call and a hand-written lookalike in the SAME suite key together: the expanded case
// takes the caller's suite path, so it pairs with the ordinary twin as duplicate-test (with
// duplicate-test-name beside it, the titles being identical). This pins the caller-side prefix
// against the engine's own composition for ordinary suite members.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L16 "s > twin works"
// @vitest-dedup:participant L24 "s > twin works"
// @vitest-dedup:expect rule=duplicate-test-name verdict=name-collision severity=warning
// @vitest-dedup:participant L16 "s > twin works"
// @vitest-dedup:participant L24 "s > twin works"
import { describe, test, expect } from 'vitest'

/** Registers one test per call, titled from the call's label. */
function makeTest(label: string) {
  test(`${label} works`, () => {
    const rows = 7
    expect(rows).toBe(3)
  })
}

describe('s', () => {
  makeTest('twin')
  test('twin works', () => {
    const rows = 7
    expect(rows).toBe(3)
  })
})
