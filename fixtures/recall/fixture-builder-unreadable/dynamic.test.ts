// RECALL negative: evaluating a builder value is outside the owned context model.
// One-edit counterfactual: replace makeValue() with 1; the duplicate becomes comparable.
import { expect, test as base } from 'vitest'

const makeValue = () => 1
const test = base.extend('value', makeValue())

test('first dynamic builder', ({ value }) => {
  expect(value).toBe(1)
})

test('second dynamic builder', ({ value }) => {
  expect(value).toBe(1)
})
