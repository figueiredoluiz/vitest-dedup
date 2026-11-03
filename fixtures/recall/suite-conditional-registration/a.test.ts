import { describe, test, expect } from 'vitest'
describe('s', () => {
  test('t', () => { expect(1).toBe(1) })
})
if (process.env.SKIP_REST) { throw new Error('stop') }
describe('s', () => {
  test('t', () => { expect(1).toBe(1) })
})
