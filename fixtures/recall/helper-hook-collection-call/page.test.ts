// RECALL negative -- duplicate-test, control for helper-hook-runtime-calls: the returned function
// is called from the suite body, while Vitest collects the file, and registers `page > extra`
// (Vitest 5.0.5 collects it). What it registers is unknown to the analyzer, so the file is refused.
import { describe, expect, test } from 'vitest'
import { setupPage } from './page'

describe('page', () => {
  const { value } = setupPage()
  value()

  test('first count', () => {
    expect(1).toBe(1)
  })

  test('second count', () => {
    expect(1).toBe(1)
  })
})
