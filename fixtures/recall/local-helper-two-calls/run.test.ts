// RECALL positive and negative -- shape: one local helper called twice with different arguments.
// The two invocations are distinct runtime cases, so their equal-argument pattern also reads as
// a consolidation candidate (one finding), beside the pair and the negative below.
//
// Each call registers its own test with `n` bound to that call's argument: `check 1` asserts what
// `one` asserts and pairs with it, while `check 2` fails and must pair with neither.
import { test, expect } from 'vitest'

function check(n: number) {
  test(`check ${n}`, () => {
    expect(n).toBe(1)
  })
}

check(1)
check(2)

test('one', () => {
  expect(1).toBe(1)
})
