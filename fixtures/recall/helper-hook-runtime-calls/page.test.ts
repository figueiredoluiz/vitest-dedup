// RECALL positive -- duplicate-test, shape: a helper registers hooks and returns functions the
// tests call. Those calls run while each test runs, after Vitest has collected the file, so they
// register nothing (Vitest 5.0.5 collects `page > first count` and `page > second count`) and the
// identical pair is compared.
import { describe, expect, test } from 'vitest'
import { setupPage } from './page'

describe('page', () => {
  const { count } = setupPage()

  test('first count', () => {
    expect(count()).toBe(1)
  })

  test('second count', () => {
    expect(count()).toBe(1)
  })
})
