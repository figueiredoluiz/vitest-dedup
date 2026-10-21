// First-party runner that is both a named export and the default export.
import { expect, test } from 'vitest'

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
export const defineSlugChecks = (label: string) => {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}

export default defineSlugChecks
