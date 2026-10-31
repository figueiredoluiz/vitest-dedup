// A call through `export * as ns`.
import { describe, expect, test } from 'vitest'
import { ns } from './barrel'

describe('namespace-barrel', () => {
  ns.cases()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
