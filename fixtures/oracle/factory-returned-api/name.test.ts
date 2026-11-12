import { expect, test } from 'vitest'

/** Returns the test API's name, a string. */
function title() {
  return test.name
}

/** Returns the test API's name through an alias. */
function aliased() {
  const n = test.name
  return n
}

const name = title()
const other = aliased()
test('a', () => { expect(typeof name + typeof other).toBe('stringstring') })
