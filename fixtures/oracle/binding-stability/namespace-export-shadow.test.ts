// An explicit export hides the same name from `export *`, and syntax cannot always tell which
// wins: the name is refused rather than read as the namespace.
import { expect, test } from 'vitest'
import { alias } from './namespace-export-shadow'

alias.test('never registered', () => {})
test('stays comparable', () => {
  expect(1).toBe(1)
})
