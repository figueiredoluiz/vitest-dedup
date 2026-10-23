// RECALL control -- duplicate-test absent, shape: a local overloaded helper reassigned before the
// call.
//
// One edit from `local-helper-overloaded`: `register` is replaced by an arrow that registers
// `replacement`, so at runtime the implementation's `old body` is never registered. Reading the
// implementation would report `old body` and `independent` as duplicates; the helper is refused
// instead and the file is set aside.
import { test, expect } from 'vitest'

/** The overload signature. */
function register(): void
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
