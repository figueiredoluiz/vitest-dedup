// RECALL positive -- duplicate-test, shape: a local helper declared with an overload signature.
//
// The call resolves to the implementation behind the signature, so `old body` is registered and is
// identical to `independent`. The sibling case reassigns `register` before the call; that one
// edit means the implementation never runs, and the pair must not be reported.
import { test, expect } from 'vitest'

/** The overload signature. */
function register(): void
/** Registers `old body`. */
function register() {
  test('old body', () => {
    expect(1).toBe(1)
  })
}

register()

test('independent', () => {
  expect(1).toBe(1)
})
