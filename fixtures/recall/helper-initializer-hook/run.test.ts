// RECALL control -- duplicate-test absent. A hook in a simple helper initializer is read; the contexts differ.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
beforeEach(() => { state.value = 0 })
const setup = () => {
  const stop = beforeEach(() => { state.value = 1 })
}

describe('hooked', () => {
  setup()
  test('reads', () => { expect(state.value).toBe(1) })
})

describe('plain', () => {
  test('reads', () => { expect(state.value).toBe(1) })
})
