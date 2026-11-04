// RECALL positive -- duplicate-test, shape: the tests call a local function of the suite that
// calls what a hooks-registering helper returned. The local runs only while those tests run, after
// Vitest has collected the file (5.0.5 collects `page > first count` and `page > second count`),
// so its calls register nothing and the identical pair is compared.
import { describe, expect, test } from 'vitest'
import { setupPage } from './page'

describe('page', () => {
  const { count } = setupPage()

  /** Reads the counter through the helper. */
  async function current() {
    return count()
  }

  test('first count', async () => {
    expect(await current()).toBe(1)
  })

  test('second count', async () => {
    expect(await current()).toBe(1)
  })
})
