import { describe, expect, test } from 'vitest'

describe.each`x
${1}`('prototype', ({ hasOwnProperty }) => {
  test('child', () => expect(typeof hasOwnProperty).toBe('function'))
})

describe.each([1, 2])('arguments %s', function () {
  test('child', () => expect(arguments[0]).toBeGreaterThan(0))
})
