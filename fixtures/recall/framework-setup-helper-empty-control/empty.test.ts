// RECALL positive -- duplicate-test, control for framework-setup-helper-shapes.
//
// An empty helper called at the top level is compared today and the identical pair is reported.
// It fails a fix that treats every top-level helper call as a refusal, and a fix for the trait
// cases that stops reporting here. Add a statement to one test body and the pair differs.
import { expect, test } from 'vitest'
import { setupEmpty } from './helper-empty'

setupEmpty()

test('empty: first check', () => {
  expect([1, 2, 3].map((n) => n * 2)).toEqual([2, 4, 6])
})

test('empty: second check', () => {
  expect([1, 2, 3].map((n) => n * 2)).toEqual([2, 4, 6])
})
