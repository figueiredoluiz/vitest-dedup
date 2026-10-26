// First-party runner whose module creates module-local values at import: a stub object, a data array
// and a class with a literal static field.
import { expect, test } from 'vitest'

export const api = { fetchUser: () => ({ name: 'Ada' }) }

export const seeds = [42, 1337] as const

export class Registry {
  static items: string[] = []
}

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
export const defineSlugChecks = (label: string) => {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}
