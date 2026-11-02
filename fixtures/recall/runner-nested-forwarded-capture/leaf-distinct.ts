import { test, expect } from 'vitest'
export default function leaf(label) {
  test(`${label} leaf`, () => {
    expect(label.trim().toLowerCase()).toBe('other')
  })
}
