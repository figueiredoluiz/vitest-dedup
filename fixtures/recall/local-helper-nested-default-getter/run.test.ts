// RECALL control -- duplicate-test absent, shape: a nested parameter default is destructured again.
//
// `{ child: { value } = options }` receives `{}`, so the default runs and is destructured: reading
// `options.value` runs a getter that registers a `beforeEach` in suite `a`. That hook is outside the
// helper's body, so the call is refused rather than compared without it.
import { describe, test, expect, beforeEach } from 'vitest'

let n = 0

describe('a', () => {
  const options = {
    get value() {
      beforeEach(() => {
        n = 1
      })
      return 0
    },
  }
  /** Declares `t` once its options bind. */
  function declare({ child: { value: _value } = options }) {
    return test('t', () => {
      expect(n).toBe(0)
    })
  }
  declare({})
})

describe('b', () => {
  test('t', () => {
    expect(n).toBe(0)
  })
})
