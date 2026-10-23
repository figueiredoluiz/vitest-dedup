// RECALL control -- duplicate-test absent, shape: a local helper declared once and reassigned
// before the call, with no overload signature.
//
// One edit from `local-helper-overloaded-reassigned`: the overload signature is gone, so the
// refusal must come from the reassignment alone. Inlining the declaration would report `old body`
// and `independent` as duplicates although the call runs the replacement and `old body` never
// registers; the helper is refused instead and the file is set aside.
import { test, expect } from 'vitest'

/** Registers `old body`, until the file reassigns it. */
function register() {
  test('old body', () => {
    expect(1).toBe(1)
  })
}

register = () => {
  test('replacement', () => {
    expect(2).toBe(2)
  })
}

register()

test('independent', () => {
  expect(1).toBe(1)
})
