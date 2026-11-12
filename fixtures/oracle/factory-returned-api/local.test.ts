import { expect, test } from 'vitest'

/** Returns an extended test API. */
function make() {
  return test.extend({ n: 1 })
}

/** Returns an extended test API asynchronously. */
async function later() {
  return test.extend({ n: 1 })
}

const it = make()
const lateIt = await later()
it('a', ({ n }) => { expect(n).toBe(1) })
lateIt('b', ({ n }) => { expect(n).toBe(1) })
