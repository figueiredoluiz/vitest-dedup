import { expect, test } from 'vitest'

/** Registers a test and returns the test API. */
function leaf() {
  test('leaf', () => { expect(1).toBe(1) })
  return test
}

/** Registers a test and returns what `leaf` returns. */
function middle() {
  test('middle', () => { expect(2).toBe(2) })
  return leaf()
}

/** Registers a test and returns what `middle` returns. */
async function outer() {
  test('outer', () => { expect(3).toBe(3) })
  return middle()
}

const it = await outer()
it('returned', () => { expect(4).toBe(4) })
