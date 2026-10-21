// RECALL variant -- duplicate-test; the run must say it could not resolve anything.
//
// `tsconfig.json` extends `./generated/tsconfig.json`, a generated file that is not on disk. The
// resolver then fails every import under this tsconfig, relative ones included, so the pair below
// cannot be compared and the run is incomplete, with the missing file named. Its sibling
// `missing-prerequisite-extends-present` is the same source with the file present, where the
// duplicate is reported and the run is complete: create `generated/tsconfig.json` (`{}`) and this
// case becomes that one.
import { test } from 'vitest'
import { checkActive } from './helpers/a'
import { assertNonEmpty } from './helpers/b'

test('accepts a plain label', () => {
  checkActive('dashboard')
})

test('accepts a plain label, again', () => {
  assertNonEmpty('dashboard')
})
