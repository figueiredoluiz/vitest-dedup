// RECALL positive -- duplicate-test, shape: a setup helper that imports an installed package's data
// file (a locale table).
//
// The helper reads `@test-kit/locale/dist/ja.json` and stores it; nothing in it can register a test.
// A data file is judged by its package manifest and the word `vitest`, not parsed as JavaScript,
// so the call is setup and the identical pair is reported. Parse the JSON as a script and the
// package is refused, the pair stays not-compared and the finding disappears.
import { expect, test } from 'vitest'
import { installLocale } from './locale-setup'

installLocale()

test('shows the required message: first check', () => {
  expect(1 + 1).toBe(2)
})

test('shows the required message: second check', () => {
  expect(1 + 1).toBe(2)
})
