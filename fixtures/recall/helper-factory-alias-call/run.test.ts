// RECALL negative -- no duplicate-test, shape: a helper registers rows through a table it built.
//
// `each('hidden %s', ...)` registers `hidden 1` and `hidden 2`; reading the call as one test
// titled `hidden %s` would compare a test Vitest never runs.
import { test, expect } from 'vitest'

function run() {
  const each = test.each([1, 2])
  each('hidden %s', () => {
    expect(1).toBe(1)
  })
}

run()

test('visible', () => {
  expect(1).toBe(1)
})
