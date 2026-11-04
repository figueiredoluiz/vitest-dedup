// RECALL negative -- duplicate-test, control for helper-hook-runtime-local: the suite body also
// calls the local, so it runs while Vitest collects the file and what it calls is unknown there.
// The analyzer keeps those calls as sites and refuses the file.
import { describe, expect, test } from 'vitest'
import { setupPage } from './page'

describe('page', () => {
  const { count } = setupPage()

  current()

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
