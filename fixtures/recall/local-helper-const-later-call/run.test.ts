// RECALL positive -- duplicate-test expected, shape: a `const` helper called from a function
// written above it, which runs only after the declarator.
//
// One edit from `local-helper-const-tdz`: `run()` moves below the declarator. The inner call is
// still written above `declare`, but it executes at `run()`, after initialization, so `hidden`
// is read and compared with `visible`.
import { test, expect } from 'vitest'

/** Calls `declare`, wherever `run` itself is called. */
function run() {
  declare()
}

const declare = () => {
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

run()

test('visible', () => {
  expect(1).toBe(1)
})
