// RECALL negative -- no duplicate-test, shape: an unused `test.each` sets state on the test chain.
//
// Building the table marks the shared chain until the table is called, so `after` is registered as
// a table case and `before` is not: `before` passes and `after` fails. Treating the construction
// as inert would pair them.
import { test, expect } from 'vitest'

function run() {
  test('before', ({ task }) => {
    expect(task.each).toBeUndefined()
  })
  const unused = test.each([1])
  test('after', ({ task }) => {
    expect(task.each).toBeUndefined()
  })
}

run()
