// RECALL positive control -- duplicate-test expected, shape: a helper called with a local computed
// by an unfollowed call, an object literal and a defaulted parameter.
//
// One edit from `local-helper-argument-hook`: the inputs only read and create values, so they run
// no code at the call and the helper's body is read; `hidden` is compared with `visible`.
import { test, expect } from 'vitest'

/** Builds a value through code the registration scan cannot follow (`eval`). */
function make() {
  return eval("({ id: 'made' })")
}

// Reading `made` at the call runs nothing, whatever built it.
const made = make()

/** Declares `hidden` whatever it is given. */
function declare(_made: unknown, _options: unknown, _suffix = '') {
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

declare(made, { made, label: 'x' })

test('visible', () => {
  expect(1).toBe(1)
})
