// RECALL positive -- duplicate-test expected, shape: a concise `const` arrow whose body is the
// registering call, with its title passed as an argument.
//
// The parameter folds to the call's argument, so the declaration renders as `hidden` and is
// compared with `visible`.
import { test, expect } from 'vitest'

const declare = (name: string) =>
  test(name, () => {
    expect(1).toBe(1)
  })

declare('hidden')

test('visible', () => {
  expect(1).toBe(1)
})
