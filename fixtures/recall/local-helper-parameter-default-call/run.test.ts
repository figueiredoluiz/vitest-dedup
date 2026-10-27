// RECALL control -- duplicate-test absent, shape: a helper's default calls the function its call passes.
//
// `hook = register()` runs the caller's arrow, which registers a `beforeEach` in suite `a`, so
// `a > t` reads a different value than `b > t`. The default's effect is not in the helper's body,
// so the call is refused rather than compared without its hook.
import { describe, test, expect, beforeEach } from 'vitest'

let n = 0

describe('a', () => {
  /** Declares `t` after running the caller's setup. */
  function declare(register, _hook = register()) {
    return test('t', () => {
      expect(n).toBe(0)
    })
  }
  declare(() =>
    beforeEach(() => {
      n = 1
    }),
  )
})

describe('b', () => {
  test('t', () => {
    expect(n).toBe(0)
  })
})
