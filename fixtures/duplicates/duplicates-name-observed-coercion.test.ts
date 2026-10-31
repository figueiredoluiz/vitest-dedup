// The same pairs with one name in both tests: nothing the coercion prints differs, so each pair is
// a real duplicate. One edit from `not-duplicates-name-observed-coercion.test.ts`.
// @vitest-dedup:extracted 8
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L18 "string one"
// @vitest-dedup:participant L24 "string two"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L30 "template one"
// @vitest-dedup:participant L36 "template two"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L42 "concat one"
// @vitest-dedup:participant L48 "concat two"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L54 "destructure one"
// @vitest-dedup:participant L61 "destructure two"
import { expect, test } from 'vitest'

test('string one', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  expect(String(alpha)).toMatchSnapshot('twin')
})

test('string two', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  expect(String(alpha)).toMatchSnapshot('twin')
})

test('template one', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  expect(`${alpha}`).toMatchSnapshot('twin')
})

test('template two', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  expect(`${alpha}`).toMatchSnapshot('twin')
})

test('concat one', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  expect(alpha + '').toMatchSnapshot('twin')
})

test('concat two', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  expect(alpha + '').toMatchSnapshot('twin')
})

test('destructure one', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  const { name } = alpha
  expect(name).toMatchSnapshot('twin')
})

test('destructure two', () => {
  /** A local name the coercion prints. */
  function alpha() {}
  const { name } = alpha
  expect(name).toMatchSnapshot('twin')
})
