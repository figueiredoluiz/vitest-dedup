// Outside the analysis root of `../root`; must never be opened through its tsconfig alias.
import { expect } from 'vitest'

export function checkActive(label: string) {
  const trimmed = label.trim()
  expect(trimmed.length).toBeGreaterThan(0)
  expect(trimmed.toUpperCase()).toBe(trimmed)
}
