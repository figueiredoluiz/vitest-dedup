// Helper module, side A of the equivalent pair. Reached only through the `~` alias.
import { expect } from 'vitest'

/** Setup helper `checkActive` of this case, called from its test file(s). */
export function checkActive(label: string) {
  const trimmed = label.trim()
  expect(trimmed.length).toBeGreaterThan(0)
  expect(trimmed.toUpperCase()).toBe(trimmed)
}
