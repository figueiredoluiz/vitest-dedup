import { describe, expect, test } from 'vitest'

let counter = 0
describe.each([1, 1])('suite %s', () => {
  test.each`
    value
    ${counter++}
  `('child $value', ({ value }) => expect(value).toBeTypeOf('number'))
})
