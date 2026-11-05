import { describe, expect, test } from 'vitest'

const LEVELS = { audit: 3 }
test.each([[LEVELS.audit], [3]])('member', n => expect(n).toBe(3))
test.each([[NaN], [0 / 0]])('nan', n => expect(n).toBeNaN())
test.each([[0], [-0]])('zero', n => expect(typeof n).toBe('number'))
test.each([[1], ['1']])('type', n => expect(n).toBeDefined())
test.each([[-0], [-0]])('decimal %d', n => expect(Object.is(n, -0)).toBe(true))
test.each([[-0], [-0]])('float %f', n => expect(Object.is(n, -0)).toBe(true))
test.each`
  value
  ${10n}
  ${10n}
`('bigint $value', ({ value }) => expect(value).toBe(10n))

describe.each([1, 2])('outer %s', outer => {
  describe.each([3, 4])('inner %s', inner => {
    test.each([5, 6])('leaf %s', leaf => {
      expect([outer, inner, leaf]).toHaveLength(3)
    })
  })
})
