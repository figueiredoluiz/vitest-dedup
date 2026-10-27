// RECALL control -- duplicate-test absent, shape: a `const` helper declared in one `switch` case and
// called from a later one.
//
// A `switch` is one scope entered at whichever case matches. Entering at `'2'` skips the
// declarator, so `declare()` reads it in its temporal dead zone and Vitest fails the file. The call
// is written after the declarator but is not in its case, so `hidden` must not be read; the call
// is refused and the file is set aside.
import { test, expect } from 'vitest'

const mode = String(2)

switch (mode) {
  case '1':
    const declare = () => {
      test('hidden', () => {
        expect(1).toBe(1)
      })
    }
    break
  case '2':
    declare()
}

test('visible', () => {
  expect(1).toBe(1)
})
