// RECALL variant -- duplicate-test; tsconfig `paths` that leave the analysis root.
//
// The alias `@h/*` maps to `../outside/*`, which is not part of the tree being analyzed. Opening
// those helpers would let a repository's configuration choose which other files on the machine the
// analyzer reads, so the imports are refused: the helper bodies are never compared, the pair is not
// reported as a duplicate, and the run is incomplete. The sibling
// `duplicate-test-tsconfig-paths-inside-root` is the same pair with the alias kept inside the root.
import { test } from 'vitest'
import { checkActive } from '@h/a'
import { assertNonEmpty } from '@h/b'

test('accepts a plain label', () => {
  checkActive('dashboard')
})

test('accepts a plain label, again', () => {
  assertNonEmpty('dashboard')
})
