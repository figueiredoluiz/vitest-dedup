// Helper module, side B: alpha-equivalent to check-a.ts with every name changed.
import { expect } from 'vitest'

/** Setup helper `assertNonEmpty` of this case, called from its test file(s). */
export function assertNonEmpty(text: string) {
  const cleaned = text.trim()
  expect(cleaned.length).toBeGreaterThan(0)
  expect(cleaned.toUpperCase()).toBe(cleaned)
}
