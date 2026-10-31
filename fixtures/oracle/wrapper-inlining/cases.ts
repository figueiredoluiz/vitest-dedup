import { expect, test } from 'vitest'

/** Registers a test the file does not spell. */
export function cases() {
  test('registered by the helper', () => {
    expect(1).toBe(1)
  })
}
