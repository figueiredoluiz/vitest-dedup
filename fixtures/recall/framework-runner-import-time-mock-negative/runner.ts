// First-party runner that imports a module which mocks `./slug` at load.
import { expect, test } from 'vitest'
import './setup-mocks'
import { slug } from './slug'

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
export const defineSlugChecks = (label: string) => {
  test(`${label} lowercases`, () => {
    expect(slug('Home-Page')).toBe('home-page')
  })
}
