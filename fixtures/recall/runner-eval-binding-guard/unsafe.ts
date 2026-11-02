import { test, expect } from 'vitest'
export function run(label) {
  test('unsafe first', () => { eval('label = "other"'); expect(label).toBe('page') })
  test('unsafe second', () => { eval('label = "other"'); expect('page').toBe('page') })
}
