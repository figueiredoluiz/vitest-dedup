// RECALL negative -- no duplicate-test, shape: a helper builds a fixture override it never uses.
//
// `test.override` installs an automatic fixture for the tests after it, which counts its runs:
// `hidden` sees 1 and passes, `visible` sees 2 and fails. Reading the override as an unused
// registration function would pair them.
import { test, expect } from 'vitest'

function run() {
  const ignored = test.override({
    counter: [async ({}, use) => {
      globalThis.counter = (globalThis.counter || 0) + 1
      await use(0)
    }, { auto: true }],
  })
  test('hidden', () => {
    expect(globalThis.counter).toBe(1)
  })
}

run()

test('visible', () => {
  expect(globalThis.counter).toBe(1)
})
