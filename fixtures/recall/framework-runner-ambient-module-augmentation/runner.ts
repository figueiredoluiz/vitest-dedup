// First-party runner whose module augments `vitest` with a type-only `declare module` block.
import { expect, test } from 'vitest'

declare module 'vitest' {
  interface Assertion<T = any> {
    toBeSlug(): T
  }
}

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
export const defineSlugChecks = (label: string) => {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}
