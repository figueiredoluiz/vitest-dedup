// RECALL variant -- duplicate-test; retention control for tsconfig path confinement.
//
// The tsconfig `paths` alias `@h/*` maps to `helpers/*` INSIDE the analysis root, so both helper
// bodies are resolved and compared: the two tests are the duplicate they look like, and the run is
// complete. Its sibling `duplicate-test-tsconfig-paths-outside-root` is the same pair with the
// alias pointing outside the root, where nothing may be opened.
import { test } from 'vitest'
import { checkActive } from '@h/a'
import { assertNonEmpty } from '@h/b'

test('accepts a plain label', () => {
  checkActive('dashboard')
})

test('accepts a plain label, again', () => {
  assertNonEmpty('dashboard')
})
