import { describe, expect, test } from 'vitest'

const S = 1
/** Returns a title the analyzer cannot read statically. */
function dynamicTitle() { return 'runtime title' }

test.for([S, S, 2])(dynamicTitle(), () => { expect(true).toBe(true) })
test.for([S, S, 2])('positive %i', () => { expect(true).toBe(true) })
test.for([S, S])('indexed %#', () => { expect(true).toBe(true) })
test.for([S, S])('unknown $missing', () => { expect(true).toBe(true) })
describe(dynamicTitle(), () => {
  test.for([S, S])('unknown suite %i', () => { expect(true).toBe(true) })
})
