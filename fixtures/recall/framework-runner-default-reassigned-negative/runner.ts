// First-party runner whose default export names a function declaration that is reassigned.
import { expect, test } from 'vitest'

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
function defineSlugChecks(label: string) {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}

defineSlugChecks = () => {}

export default defineSlugChecks
