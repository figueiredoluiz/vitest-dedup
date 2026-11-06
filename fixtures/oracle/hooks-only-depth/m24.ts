import { test } from 'vitest'

/** Registers a test past the depth limit. */
export function f24() {
  test('registered past the limit', () => {})
}
