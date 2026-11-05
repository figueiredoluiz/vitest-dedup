// Two identical `beforeEach` hooks whose whole body is one own-property write over a receiver
// the module proves plain: `audit` is assigned nowhere, its initializer is an object literal
// with static data properties, every write to `audit.mode` is the same literal, and the file
// gives the object no prototype-setter route. The write runs no user code and keeps one value,
// so the repetition is redundant on the same terms as a constant lexical write. The uncertain
// twin (not-redundant-hook-property-uncertain.test.ts) adds a delete on the tracked path and
// must stay silent.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=redundant-hook verdict=redundant severity=warning
// @vitest-dedup:participant L16 "beforeEach"
// @vitest-dedup:participant L20 "beforeEach"
import { describe, test, expect, beforeEach } from 'vitest'

const audit = { mode: '' }

beforeEach(() => {
  audit.mode = 'primary'
})

beforeEach(() => {
  audit.mode = 'primary'
})

test('reads the mode', () => {
  expect(audit.mode).toBe('primary')
})
