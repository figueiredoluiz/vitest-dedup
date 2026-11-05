// RECALL negative -- no duplicate-test, shape: a helper title reads a `const` declared after the
// first call. `run()` runs the hoisted helper before `n` is initialized, so Vitest fails the file;
// reading the title as `hidden 1` would pair it with `hidden 1` below.
import { test, expect } from 'vitest'

run()

const n = 1

function run() {
  test(`hidden ${n}`, () => {
    expect(1).toBe(1)
  })
}

test('hidden 1', () => {
  expect(1).toBe(1)
})
