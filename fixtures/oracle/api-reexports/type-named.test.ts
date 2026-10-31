// A type-only specifier exports no value, so this `test` is the star's plain function.
// The named callback would be a recorded exclusion if the call were read as a test.
import { expect, test as visible } from 'vitest'
import { test } from './type-named'

/** Never runs. */
function run() {}

test('never registered', run)
visible('stays comparable', () => {
  expect(1).toBe(1)
})
