// A describe body runs at collection, so setup in it reaches every test of the file: the two files
// run the same test against different modes. Named weakening: make the two `mode` values equal --
// the files are then the same and the pair is a real duplicate.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates context
// @vitest-dedup:isolation-violation no context key sees setup a describe body runs at collection
// @vitest-dedup:pending setup a describe body runs at collection is not part of the module context
// @vitest-dedup:participant L15 "reads the collection-time mode"
// @vitest-dedup:participant not-cross-file-suite-collection-setup-a.test.ts#L15 "reads the collection-time mode"
import { describe, expect, test } from 'vitest'

describe('setup', () => { globalThis.collectionMode = 'beta' })

test('reads the collection-time mode', () => {
  expect(globalThis.collectionMode).toBeTruthy()
})
