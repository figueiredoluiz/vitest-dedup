// First-party runner exported as `const f = ...; export default f`.
import { expect, test } from 'vitest'

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
const defineSlugChecks = (label: string) => {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}

export default defineSlugChecks
