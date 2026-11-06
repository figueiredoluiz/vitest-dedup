// RECALL negative -- no duplicate-test, shape: a table builder coerces an argument that runs code.
//
// An array with a further argument is read as a template table and joined, which calls the
// object's `toString`; it throws, so Vitest fails the file. Treating the construction as inert
// would pair `hidden` with `visible`.
import { test, expect } from 'vitest'

function run() {
  const unused = test.for([{ toString() { throw new Error('coerced') } }], 1)
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

run()

test('visible', () => {
  expect(1).toBe(1)
})
