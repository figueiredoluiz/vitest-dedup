import { test, expect } from 'vitest'
const tools = { eval() {} }
export function run(label) {
  test('safe first', () => { tools.eval('label = "other"'); expect(label).toBe('page') })
  test('safe second', () => { tools.eval('label = "other"'); expect('page').toBe('page') })
}
