// Two suites whose concise bodies differ only in the fixture value they scope: the sequence is the
// suite's setup, so the tests run against different values. Named weakening: make the two
// `scoped` values equal -- the suites are then the same and the pair is a real duplicate.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable the default engine extracts nothing from a concise suite body
// @vitest-dedup:participant L13 "alpha > reads the mode"
// @vitest-dedup:participant L17 "beta > reads the mode"
import { describe, expect, test } from 'vitest'

const extended = test.extend({ mode: 'base' })

describe('alpha', () => (extended.scoped({ mode: 'a' }), extended('reads the mode', ({ mode }) => {
  expect(mode.length).toBe(1)
})))

describe('beta', () => (extended.scoped({ mode: 'b' }), extended('reads the mode', ({ mode }) => {
  expect(mode.length).toBe(1)
})))

// The default engine extracts nothing from a concise suite body; this keeps the file analyzable.
test('stays comparable', () => {
  expect('concise').toHaveLength(7)
})
