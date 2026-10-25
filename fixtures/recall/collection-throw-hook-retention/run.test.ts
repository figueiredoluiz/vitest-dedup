// A throw nothing catches during collection fails the whole file, so the conditional throws in the
// module, the suite bodies, the loop and the helper cannot skip the hooks registered after them.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

/** Always throws. */
function fail() {
  throw new Error('no PATH')
}

/** Its return ends only the helper and its throw fails the file: neither skips a caller hook. */
function check(n) {
  if (!n) return
  if (n > 1) throw new Error('too many')
}

if (!process.env.PATH) throw new Error('no PATH')

describe('suite-a', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  if (!process.env.PATH) throw new Error('no PATH')
  if (!process.env.PATH) {
    throw new Error('no PATH')
  }
  for (const n of [1]) {
    if (n > 1) throw new Error('too many')
  }
  if (!process.env.PATH) fail()
  check(1)
  beforeEach(() => {
    state.n = 1
  })
})

describe('suite-b', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  if (!process.env.PATH) throw new Error('no PATH')
  if (!process.env.PATH) {
    throw new Error('no PATH')
  }
  for (const n of [1]) {
    if (n > 1) throw new Error('too many')
  }
  if (!process.env.PATH) fail()
  check(1)
  beforeEach(() => {
    state.n = 1
  })
})
