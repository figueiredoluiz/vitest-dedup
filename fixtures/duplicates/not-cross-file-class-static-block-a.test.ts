// A class expression's static block runs at load, so the two files run the same test against
// different modes. Named weakening: make the two `staticMode` values equal -- the files are then
// the same and the pair is a real duplicate.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates context
// @vitest-dedup:isolation-violation the default engine's context key does not see a static block
// @vitest-dedup:pending the default engine does not read a class expression's static block as load effects
// @vitest-dedup:participant L16 "reads the mode a static block set"
// @vitest-dedup:participant not-cross-file-class-static-block-b.test.ts#L16 "reads the mode a static block set"
import { expect, test } from 'vitest'

const setup = class { static { globalThis.staticMode = 'alpha' } }
void setup

test('reads the mode a static block set', () => {
  expect(globalThis.staticMode).toBeTruthy()
})
