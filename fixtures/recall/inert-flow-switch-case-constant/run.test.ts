// RECALL negative -- no duplicate-test, shape: a suite reads a `const` its `switch` case skipped.
//
// Entering at `case true` skips the declaration of `on`, so reading it in suite `a` throws and
// Vitest fails the file. Treating the read as a constant would pair `with` and `without`.
import { describe, test, expect } from 'vitest'

switch (true) {
  case false:
    const on = true
  case true:
    describe('a', () => {
      test('with', () => {
        expect(1).toBe(1)
      })
      if (on) {}
    })
    describe('b', () => {
      test('without', () => {
        expect(1).toBe(1)
      })
    })
}
