// Two suites that differ ONLY in an enclosing `.skipIf` condition, each containing one nested
// test with an identical body.
// Expected verdict: NOT a duplicate. A suite's own `each`/`skipIf`/`runIf` reaches its nested
// tests through their context. A suite that pushed its title and hook scope but dropped its own
// modifiers would give both nested tests the same context, and this pair would collapse into an
// error-level false duplicate despite running under genuinely different conditions.
//
// (Suite/test titles are deliberately distinct so `duplicate-test-name` has nothing to say about
// this pair -- the fix under test is `duplicate-test`'s modifier key, not name matching.)
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates context
// @vitest-dedup:participant L21 "feature one > does the thing"
// @vitest-dedup:participant L27 "feature two > does the thing"
import { describe, test, expect } from 'vitest'

declare const FEATURE_A: boolean
declare const FEATURE_B: boolean

describe.skipIf(FEATURE_A)('feature one', () => {
  test('does the thing', () => {
    expect(1 + 1).toBe(2)
  })
})

describe.skipIf(FEATURE_B)('feature two', () => {
  test('does the thing', () => {
    expect(1 + 1).toBe(2)
  })
})
