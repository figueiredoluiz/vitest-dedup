import { test, expect } from 'vitest'

/** Registers `hidden` once per call. */
export function register() {
  test('hidden', () => {
    expect(1).toBe(1)
  })
}
