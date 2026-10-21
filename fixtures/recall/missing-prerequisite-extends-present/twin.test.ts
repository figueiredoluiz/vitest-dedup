// RECALL variant -- duplicate-test; retention control for the missing-prerequisite notice.
//
// Same source as `missing-prerequisite-extends-absent`, but the extended `generated/tsconfig.json`
// exists, so every import resolves, the two tests are the duplicate they look like, and the run is
// complete. It fails the notice if it fires for a prerequisite that is present.
import { test } from 'vitest'
import { checkActive } from './helpers/a'
import { assertNonEmpty } from './helpers/b'

test('accepts a plain label', () => {
  checkActive('dashboard')
})

test('accepts a plain label, again', () => {
  assertNonEmpty('dashboard')
})
