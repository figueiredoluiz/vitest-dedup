import { describe, expect, test } from 'vitest'

let plain = 'plain let'
test(plain, () => { expect(plain).toBe('plain let') })

let renamed = 'first'
renamed = 'second'
test(renamed, () => { expect(renamed).toBe('second') })

let astral = '😀-😀𝒳'
astral = astral.replaceAll('😀', 'é')
test(astral, () => { expect(astral).toHaveLength(5) })

describe.each([['x'], ['y']])('%s', (key) => {
  let suffix = 'base'
  if (key !== 'x') {
    suffix = `${suffix}-other`
  } else {
    suffix = `${suffix}-x`
  }
  test(suffix, () => { expect(key).toMatch(/^[xy]$/) })
  test(key, () => { expect(key.length).toBe(1) })
  test(`${key} as template`, () => { expect(`${key}!`).toContain(key) })
})

let moduleTitle = 'module before'
describe('module', () => {
  test(moduleTitle, () => { expect('module').toHaveLength(6) })
})
await Promise.resolve()
moduleTitle = 'module after'

describe('outer', () => {
  let nested = 'nested before'
  describe('inner', () => {
    test(nested, () => { expect('nested').toHaveLength(6) })
  })
  nested = 'nested after'
})
