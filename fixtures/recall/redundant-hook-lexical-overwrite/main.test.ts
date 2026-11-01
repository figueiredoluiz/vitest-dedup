// A primitive lexical overwrite is idempotent; an intervening observer remains a barrier.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=redundant-hook verdict=redundant severity=warning
// @vitest-dedup:participant L14 "beforeEach"
// @vitest-dedup:participant L15 "beforeEach"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:participant L21 "beforeEach"
// @vitest-dedup:participant L23 "beforeEach"
import { beforeEach, describe, expect, test } from 'vitest'
let hookValue = 0
let barrierValue = 0
let observedHookValue = 0
describe('retained lexical overwrite', () => {
  beforeEach(() => { hookValue = 31 })
  beforeEach(() => { hookValue = 31 })
  test('keeps the owned lexical value', () => {
    expect({ hookValue, marker: 'owned hook constant' }).toEqual({ hookValue: 31, marker: 'owned hook constant' })
  })
})
describe('retained hook observer', () => {
  beforeEach(() => { barrierValue = 1 })
  beforeEach(() => { observedHookValue = barrierValue })
  beforeEach(() => { barrierValue = 1 })
  test('observes the first overwrite', () => {
    expect([barrierValue, observedHookValue]).toEqual([1, 1])
  })
})
