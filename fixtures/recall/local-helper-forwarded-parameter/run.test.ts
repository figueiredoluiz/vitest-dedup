// RECALL control -- duplicate-test absent, shape: a helper destructures a value forwarded by a parameter.
//
// `forward(options)` passes its parameter on to `declare({ value })`, which reads `options.value`:
// that getter registers a `beforeEach` in suite `hooked`. The parameter's value is not shown where
// `declare` reads it, so the call is refused rather than compared without the hook.
import { describe, test, expect, beforeEach } from 'vitest'

let n = 0

const options = {
  get value() {
    beforeEach(() => {
      n = 1
    })
    return 0
  },
}

/** Declares `reads` once its options bind. */
function declare({ value: _value }) {
  test('reads', () => {
    expect(n).toBe(0)
  })
}

/** Forwards its options to `declare`. */
function forward(input) {
  declare(input)
}

describe('hooked', () => {
  forward(options)
})

describe('plain', () => {
  test('reads', () => {
    expect(n).toBe(0)
  })
})
