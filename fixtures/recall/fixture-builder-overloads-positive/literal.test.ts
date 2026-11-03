// RECALL positive: a literal builder value is readable without evaluating project code.
// One-edit counterfactual: replace 1 with makeValue(); the file becomes unresolved.
import { expect, test as base } from 'vitest'

const test = base.extend('value', 1)

test('first literal builder', ({ value }) => {
  expect(value).toBe(1)
})

test('second literal builder', ({ value }) => {
  expect(value).toBe(1)
})
