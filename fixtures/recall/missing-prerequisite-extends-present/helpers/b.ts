// Helper reached by a relative import; alpha-equivalent to its sibling.
import { expect } from 'vitest'

/** Checks the label is not empty. */
export function assertNonEmpty(text: string) {
  const cleaned = text.trim()
  expect(cleaned.length).toBeGreaterThan(0)
  expect(cleaned.toUpperCase()).toBe(cleaned)
}
