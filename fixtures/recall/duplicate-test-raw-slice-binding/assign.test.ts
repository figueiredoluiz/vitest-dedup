// RECALL negative with a positive control -- a destructuring assignment target is kept as raw
// source text, so its names have to be tied to the variables they mean.
//
// In the first test `[x] = [1]` assigns the variable declared first and the assertion reads the one
// declared second; in the second test the declarations are swapped, so the same assignment hits the
// variable the assertion reads. The raw text `[x]` and every other fragment are identical, yet the
// first test passes and the second fails -- the difference a `duplicate-test` finding would hide.
// The third test repeats the first exactly, so a detector that stopped comparing raw text would
// fail on the control instead of passing on the negative.
//
// Named weakening: declare `y` first in the first test and its pair with the second becomes a
// genuine duplicate. The second test fails when run, on purpose: that is the behavior at stake.
import { expect, test } from 'vitest'

test('assigns the first variable and reads the second', () => {
  let x = 0
  let y = 0
  ;[x] = [1]
  expect(y).toBe(0)
})

test('assigns the second variable and reads it', () => {
  let y = 0
  let x = 0
  ;[x] = [1]
  expect(x).toBe(0)
})

test('assigns the first variable and reads the second again', () => {
  let x = 0
  let y = 0
  ;[x] = [1]
  expect(y).toBe(0)
})
