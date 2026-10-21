// RECALL positive -- duplicate-test, shape: alias from a generated tsconfig reached through `extends`.
//
// `tsconfig.json` only extends `.nuxt/tsconfig.json`, whose `paths` map `~` and `~/*` to the parent
// directory (the analysis root). Both helpers resolve through the alias, so the pair is the
// duplicate it looks like. If the alias is not followed through `extends`, the imports stay
// unresolved and the pair is not compared. Point the second import at a different module and the
// pair differs.
import { test } from 'vitest'
import { checkActive } from '~/utils/check-a'
import { assertNonEmpty } from '~/utils/check-b'

test('accepts a plain label', () => {
  checkActive('DASHBOARD')
})

test('accepts a plain label, again', () => {
  assertNonEmpty('DASHBOARD')
})
