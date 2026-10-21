// Helper reached by a relative import; alpha-equivalent to its sibling.
import { expect } from 'vitest'

/** Trims and checks the label. */
export function checkActive(label: string) {
  const trimmed = label.trim()
  expect(trimmed.length).toBeGreaterThan(0)
  expect(trimmed.toUpperCase()).toBe(trimmed)
}
