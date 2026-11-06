// RECALL negative -- no duplicate-test, shape: a helper builds a table from an alias declared after it.
//
// `later.each` reads `later` before its declaration runs, so Vitest fails the file at collection.
// Treating the construction as inert would pair `hidden` with `visible`.
import { test, expect } from 'vitest'

function run() {
  const each = later.each([1])
  const later = test
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

run()

test('visible', () => {
  expect(1).toBe(1)
})
