import { test, expect } from 'vitest'
const first = 1
const second = 2
const hidden = 'other'
export default function leaf() {
  test('other fixed', () => { expect(hidden.trim()).not.toBe('neither') })
  test('page literal', () => { expect('page'.trim()).not.toBe('neither') })
}
