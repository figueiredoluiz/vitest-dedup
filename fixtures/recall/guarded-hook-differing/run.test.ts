// RECALL control -- duplicate-test absent, shape: two identical suites whose identical hooks
// sit under opposite `if` conditions.
//
// Walking both branches without the condition made the two registrations equal: `if (true)` and
// `if (false)` produced the same hook fingerprint, and the two tests -- only one of which can
// ever see the hook run -- were reported as error-severity duplicates. A guarded registration
// now makes its scope's contexts incomparable, so the pair stays apart; the twin pins that the
// same refusal covers identical guards.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }

describe('suite-a', () => {
  if (true) beforeEach(() => {
    state.n = 1
  })
  test('reads', () => {
    expect(state.n).toBe(1)
  })
})

describe('suite-b', () => {
  if (false) beforeEach(() => {
    state.n = 1
  })
  test('reads', () => {
    expect(state.n).toBe(1)
  })
})
