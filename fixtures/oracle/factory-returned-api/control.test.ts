import { expect, test } from 'vitest'
import { joined, run } from './factories'

/** Builds an API only inside a nested arrow. */
function inner() {
  const nested = () => test.extend({})
  return false
}

/** Returns a value that is no test API. */
function data() {
  return { n: 1 }
}

/** Reports whether the test API is a function. */
function supports() {
  return typeof test === 'function'
}

/** Returns a number; the second return never runs. */
function later() {
  return 1
  return test.extend({})
}

supports()
later()
joined()

test('a', async () => {
  const { stderr, exitCode } = await run()
  expect(stderr).toBe('')
  expect(exitCode).toBe(0)
})
test('b', () => { expect(inner()).toBe(false) })
test('c', () => { expect(data().n).toBe(1) })
