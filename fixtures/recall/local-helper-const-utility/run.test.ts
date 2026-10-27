// RECALL positive control -- duplicate-test expected, shape: a local `const` helper called at
// collection time that registers nothing.
//
// One edit from `local-helper-const-arrow`: the helper fills an array instead of declaring a
// test, which is proved, so the call is not refused and the duplicate pair stays comparable.
import { test, expect } from 'vitest'

const values: number[] = []
const fill = () => {
  values.push(1)
}

fill()

test('first', () => {
  expect(values).toEqual([1])
})

test('second', () => {
  expect(values).toEqual([1])
})
