// First-party runner whose default export names a `let` binding, which is not an admitted runner shape.
import { expect, test } from 'vitest'

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
let defineSlugChecks = (label: string) => {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}

export default defineSlugChecks
