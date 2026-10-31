import { test } from 'vitest'

/** Registers a test past the depth limit. */
export function f11() {
  test('registered past the limit', () => {})
}
