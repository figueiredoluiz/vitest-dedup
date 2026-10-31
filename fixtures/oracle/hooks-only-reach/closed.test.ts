// Controls: helpers that only reach hooks, directly, recursively and through a barrel.
import { describe, expect, test } from 'vitest'
import { reexported } from './barrel'
import { pairOne, setup } from './setup'

describe('closed', () => {
  setup()
  pairOne(true)
  reexported(true)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
