// Each pair differs only in a local function's name, which the coercion prints, so the snapshots
// differ. Named weakening: use one name in both tests -- `duplicates-name-observed-coercion.test.ts`
// pins that the pair is then a real duplicate.
// @vitest-dedup:extracted 8
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable a local name, which the normal form renames
// @vitest-dedup:pending the default engine's normal form renames a name the coercion prints
// @vitest-dedup:participant L27 "string alpha"
// @vitest-dedup:participant L33 "string gamma"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable a local name, which the normal form renames
// @vitest-dedup:pending the default engine's normal form renames a name the coercion prints
// @vitest-dedup:participant L39 "template alpha"
// @vitest-dedup:participant L45 "template gamma"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable a local name, which the normal form renames
// @vitest-dedup:pending the default engine's normal form renames a name the coercion prints
// @vitest-dedup:participant L51 "concat alpha"
// @vitest-dedup:participant L57 "concat gamma"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable a local name, which the normal form renames
// @vitest-dedup:pending the default engine's normal form renames a name the coercion prints
// @vitest-dedup:participant L63 "destructure alpha"
// @vitest-dedup:participant L70 "destructure gamma"
import { expect, test } from 'vitest'

test('string alpha', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  expect(String(alpha)).toMatchSnapshot()
})

test('string gamma', () => {
  /** A local name the coercion prints. */
  function gamma() {}
  expect(String(gamma)).toMatchSnapshot()
})

test('template alpha', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  expect(`${alpha}`).toMatchSnapshot()
})

test('template gamma', () => {
  /** A local name the coercion prints. */
  function gamma() {}
  expect(`${gamma}`).toMatchSnapshot()
})

test('concat alpha', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  expect(alpha + '').toMatchSnapshot()
})

test('concat gamma', () => {
  /** A local name the coercion prints. */
  function gamma() {}
  expect(gamma + '').toMatchSnapshot()
})

test('destructure alpha', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  const { name } = alpha
  expect(name).toMatchSnapshot()
})

test('destructure gamma', () => {
  /** A local name the coercion prints. */
  function gamma() {}
  const { name } = gamma
  expect(name).toMatchSnapshot()
})
