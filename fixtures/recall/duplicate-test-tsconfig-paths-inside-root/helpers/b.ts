// Helper reached through the tsconfig alias `@h/b`; alpha-equivalent to `./a.ts`.
import { expect } from 'vitest'

export function assertNonEmpty(text: string) {
  const cleaned = text.trim()
  expect(cleaned.length).toBeGreaterThan(0)
  expect(cleaned.toUpperCase()).toBe(cleaned)
}
