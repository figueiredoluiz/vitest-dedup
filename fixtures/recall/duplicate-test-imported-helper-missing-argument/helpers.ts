// Helper module: two exports with the same body under different names, each taking two
// parameters. A call that passes one argument leaves the second `undefined` in both, so the two
// calls still do the same thing.
import { expect } from 'vitest'

/** Asserts the trimmed label is longer than `minimum` and upper case. */
export function checkActive(label: string, minimum: number) {
  const trimmed = label.trim()
  expect(trimmed.length).toBeGreaterThan(minimum)
  expect(trimmed.toUpperCase()).toBe(trimmed)
}

/** Asserts the trimmed text is longer than `floor` and upper case. */
export function assertNonEmpty(text: string, floor: number) {
  const cleaned = text.trim()
  expect(cleaned.length).toBeGreaterThan(floor)
  expect(cleaned.toUpperCase()).toBe(cleaned)
}
