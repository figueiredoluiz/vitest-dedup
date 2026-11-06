// RECALL negative -- a nested helper reads a constant declared after the top-level call.
//
// `run()` runs while the module body runs, and reaches `middle` only through another helper:
// `middle` reads `label` then, before `const label` is initialized, so Vitest fails collecting
// this file ("Cannot access 'label' before initialization"). Nothing it would register is
// compared with `copy`.
//
// Load-bearing: move `run()` below `const label` and the expansion pairs with `copy`.
import { expect, test } from 'vitest'

run()
const label = 'late'

/** Registers through `middle`. */
function run() {
  middle()
}

/** Registers one test titled from `label`. */
function middle() {
  test(label, () => {
    expect(1).toBe(1)
  })
}

test('copy', () => {
  expect(1).toBe(1)
})
