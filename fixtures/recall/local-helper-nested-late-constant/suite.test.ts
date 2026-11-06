// RECALL positive -- the same nested helper called inside a suite.
//
// A `describe` callback runs after the whole module body (Vitest 5.0.5), so `label` is
// initialized by the time `middle` reads it: the expanded `s > late` pairs with `s > copy`.
import { describe, expect, test } from 'vitest'

describe('s', () => {
  run()
  test('copy', () => {
    expect(1).toBe(1)
  })
})
const label = 'late'

/** Registers through `middle`. */
function run() {
  middle()
}

/** Registers one test titled from `label`. */
function middle() {
  test(label, () => {
    expect(1).toBe(1)
  })
}
