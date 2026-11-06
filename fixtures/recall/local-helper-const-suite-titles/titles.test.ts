// RECALL variant -- helper calls under suites titled from constants.
//
// The suite titles render from `const` bindings, so the expanded tests are named `left > shared`
// and `right > shared`, as Vitest names them: no name collision. Their bodies are equal under the
// same context, so they are one `duplicate-test`, as two handwritten copies would be.
//
// Load-bearing: give both suites the title `left` and the pair also collides by name.
import { describe, expect, test } from 'vitest'

const left = 'left'
const right = 'right'

/** Registers one shared test. */
function makeTest() {
  test('shared', () => {
    expect(1).toBe(1)
  })
}

describe(left, () => {
  makeTest()
})

describe(right, () => {
  makeTest()
})
