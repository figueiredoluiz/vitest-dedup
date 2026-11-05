import { describe, expect, test } from 'vitest'

test.each`value`('each no cells', value => expect(value).toBe('value'))
test.for`value`('for no cells', value => expect(value).toBe('value'))
describe.each`value`('each suite', value => {
  test('child', () => expect(value).toBe('value'))
})
describe.for`value`('for suite', value => {
  test('child', () => expect(value).toBe('value'))
})
test.each``('empty string', value => expect(value).toBe(''))
describe.each``('empty suite', value => {
  test('child', () => expect(value).toBe(''))
})
