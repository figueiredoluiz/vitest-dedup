import { expect, test } from 'vitest'

/** Returns an extended test API. */
function factory() {
  return test.extend({})
}

/** Returns the factory's name, a string. */
function label() {
  return factory.name
}

const name = label()
test('a', () => { expect(typeof name).toBe('string') })
test('b', () => { expect(typeof name).toBe('string') })
