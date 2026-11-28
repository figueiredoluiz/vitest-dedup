import * as v from 'vitest'
import { describe, expect, test } from 'vitest'

describe.skipIf(true)('literal skip', () => {
  test('small', () => { expect(11).toBe(11) })
  test('large', () => { expect(11).toBe(11); expect(12).toBe(12) })
})

describe.runIf(false)('literal run disabled', () => {
  describe('inner', () => {
    test('small', () => { expect(21).toBe(21) })
    test('large', () => { expect(21).toBe(21); expect(22).toBe(22) })
  })
})

const flag = Number('0') > 1
describe.skipIf(flag)('dynamic skip', () => {
  test('small', () => { expect(31).toBe(31) })
  test('large', () => { expect(31).toBe(31); expect(32).toBe(32) })
})

describe.skipIf(false)('literal skip disabled', () => {
  test('small', () => { expect(41).toBe(41) })
  test('large', () => { expect(41).toBe(41); expect(42).toBe(42) })
})

v.describe.runIf(true)('namespace run enabled', () => {
  describe('inner', () => {
    test('small', () => { expect(51).toBe(51) })
    test('large', () => { expect(51).toBe(51); expect(52).toBe(52) })
  })
})

describe('plain callback', () => {
  test('small', () => { expect(61).toBe(61) })
  test('large', () => { expect(61).toBe(61); expect(62).toBe(62) })
})

describe.runIf()('missing run condition', () => {
  test('small', () => { expect(71).toBe(71) })
  test('large', () => { expect(71).toBe(71); expect(72).toBe(72) })
})
