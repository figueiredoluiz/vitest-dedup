// `vi.mock` replaces a hooks-only helper with one that registers.
import { describe, expect, test, vi } from 'vitest'
import { setup } from './hooks'

vi.mock('./hooks', async () => ({ setup: (await import('./support')).cases }))

describe('mocked-helper', () => {
  setup()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
