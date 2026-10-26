// First-party runner whose module holds a NON-ambient namespace that registers a hook at import.
import { beforeEach, expect, test } from 'vitest'

namespace SlugSetup {
  beforeEach(() => {
    ;(globalThis as any).slugRuns = ((globalThis as any).slugRuns ?? 0) + 1
  })
}

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
export const defineSlugChecks = (label: string) => {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}
