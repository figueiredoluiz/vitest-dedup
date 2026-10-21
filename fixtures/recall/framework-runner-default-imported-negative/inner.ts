import { expect, test } from 'vitest'

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
export default function defineSlugChecks(label: string) {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}
