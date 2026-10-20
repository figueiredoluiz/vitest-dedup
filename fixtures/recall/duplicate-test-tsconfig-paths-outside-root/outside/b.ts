// Outside the analysis root of `../root`; must never be opened through its tsconfig alias.
import { expect } from 'vitest'

export function assertNonEmpty(text: string) {
  const cleaned = text.trim()
  expect(cleaned.length).toBeGreaterThan(0)
  expect(cleaned.toUpperCase()).toBe(cleaned)
}
