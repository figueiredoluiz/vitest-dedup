// RECALL positive: literal builder options and an inline factory are readable together.
// One-edit counterfactual: replace true with auto; the file becomes unresolved.
import { expect, test as base } from 'vitest'

const test = base.extend('value', { auto: true }, ({}) => 1)

test('first options builder', ({ value }) => {
  expect(value).toBe(1)
})

test('second options builder', ({ value }) => {
  expect(value).toBe(1)
})
