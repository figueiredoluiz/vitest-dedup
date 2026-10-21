// RECALL variant -- duplicate-test, shape: generated tsconfig whose alias target is outside the root.
//
// Analyzed from `app/`, the `~` alias (from `../.nuxt/tsconfig.json`) points at the directory above
// the analysis root, where the helpers live. Nothing outside the root is opened, so both imports
// stay unresolved with different identities: the pair is silent and the run reports incomplete
// without naming why. The run should name the alias target as the reason.
// Analyze the parent directory instead and it becomes framework-tsconfig-extends-generated.
import { test } from 'vitest'
import { checkActive } from '~/utils/check-a'
import { assertNonEmpty } from '~/utils/check-b'

test('accepts a plain label', () => {
  checkActive('DASHBOARD')
})

test('accepts a plain label, again', () => {
  assertNonEmpty('DASHBOARD')
})
