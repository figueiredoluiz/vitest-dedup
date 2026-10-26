// First-party runner whose module destructures an object literal with a getter, which runs at import
// and registers a test of its own.
import { expect, test } from 'vitest'

export const { label } = {
  get label() {
    test('registered while the runner module loads', () => {
      expect(1).toBe(1)
    })
    return 'page'
  },
}

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
export const defineSlugChecks = (name: string) => {
  test(`${name} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}
