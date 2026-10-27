// RECALL control -- duplicate-test absent, shape: a `const` helper reached through a hoisted
// function before its declarator runs.
//
// `run()` executes `declare()` while `declare` is still in its temporal dead zone, so Vitest fails
// the file and registers nothing. The call is written after the declarator, but it executes at
// `run()`, so `hidden` must not be read; the call is refused and the file is set aside.
import { test, expect } from 'vitest'

run()

const declare = () => {
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

/** Calls `declare`, wherever `run` itself is called. */
function run() {
  declare()
}

test('visible', () => {
  expect(1).toBe(1)
})
