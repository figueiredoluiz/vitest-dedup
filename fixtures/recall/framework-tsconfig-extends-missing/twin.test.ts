// RECALL variant -- duplicate-test, shape: `extends` target (a generated file) does not exist.
//
// The same source as framework-tsconfig-extends-generated without `.nuxt/tsconfig.json`, which a
// framework command generates and a fresh checkout lacks. The `~` alias cannot be known, both
// helper imports stay unresolved with different identities, so the two tests are silently judged
// distinct, and the run reports incomplete and names the missing file with a fix to apply (with Nuxt
// evidence in the project it is `nuxt prepare`; here it is to generate or restore the file). Add the
// generated file and it becomes the generated case.
import { test } from 'vitest'
import { checkActive } from '~/utils/check-a'
import { assertNonEmpty } from '~/utils/check-b'

test('accepts a plain label', () => {
  checkActive('DASHBOARD')
})

test('accepts a plain label, again', () => {
  assertNonEmpty('DASHBOARD')
})
