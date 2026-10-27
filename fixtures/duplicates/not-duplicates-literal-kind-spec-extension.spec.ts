// Two tests whose bodies differ only in the KIND of a literal: number `7` vs string `'7'`.
// Expected verdict: NOT a duplicate. The file is a `.spec.ts` on purpose: the isolation gate must
// enumerate every test-named extension, not only `.test.ts`, so this negative has to be counted
// among the checked expectations.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates literal-kind
// @vitest-dedup:participant L13 "reads the slot by number"
// @vitest-dedup:participant L17 "reads the slot by string"
import { test, expect } from 'vitest'
import { readSlot } from './slots'

test('reads the slot by number', () => {
  expect(readSlot(7)).toBe(true)
})

test('reads the slot by string', () => {
  expect(readSlot('7')).toBe(true)
})
