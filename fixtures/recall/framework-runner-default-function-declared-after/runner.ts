// First-party runner exported as `export default name` before `function name` is declared.
import { expect, test } from 'vitest'

export default defineSlugChecks

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
function defineSlugChecks(label: string) {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}
