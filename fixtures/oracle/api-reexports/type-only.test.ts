// A type-only star exports no value, so this `test` is the plain function: its call is no test.
// The named callback would be a recorded exclusion if the call were read as a test.
import { expect, test as visible } from 'vitest'
import { test } from './type-only'

/** Never runs. */
function run() {}

test('never registered', run)
visible('stays comparable', () => {
  expect(1).toBe(1)
})
