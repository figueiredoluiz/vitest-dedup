// This `const` hides the star's alias: the name is refused, not read as the namespace.
import { expect, test } from 'vitest'
import { alias } from './namespace-export-const'

alias.test('never registered', () => {})
test('stays comparable', () => {
  expect(1).toBe(1)
})
