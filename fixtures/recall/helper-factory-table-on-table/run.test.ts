// RECALL negative -- no duplicate-test, shape: a helper builds a table on a table.
//
// `test.for([1])` returns a plain function with no `for` member, so Vitest fails the file at
// collection. Treating the construction as inert would pair `hidden` with `visible`.
import { test, expect } from 'vitest'

function run() {
  const unused = test.for([1]).for([2])
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

run()

test('visible', () => {
  expect(1).toBe(1)
})
