// First-party runner whose module switches Vitest to fake timers while it is imported.
import { expect, test, vi } from 'vitest'

export const clock = vi.useFakeTimers()

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
export const defineSlugChecks = (label: string) => {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}
