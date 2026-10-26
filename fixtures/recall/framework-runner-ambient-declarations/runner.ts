// First-party runner whose module holds every other ambient declaration form.
import { expect, test } from 'vitest'

declare global {
  var slugSeparator: string
}

declare namespace SlugTypes {
  const separator: string
}

export declare namespace SlugShapes {
  interface Slug {
    value: string
  }
}

declare enum SlugCase {
  Lower,
  Upper,
}

/** Setup helper `defineSlugChecks` of this case, called from its test file(s). */
export const defineSlugChecks = (label: string) => {
  test(`${label} lowercases`, () => {
    const slug = 'Home-Page'.toLowerCase()
    expect(slug).toBe('home-page')
  })
}
