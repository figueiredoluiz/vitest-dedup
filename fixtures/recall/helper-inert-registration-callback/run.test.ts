// RECALL positive -- duplicate-test expected. Creating an unused callback registers no hook; the duplicate must survive.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
beforeEach(() => { state.value = 0 })
const setup = () => {
  const deferred = () => beforeEach(() => { state.value = 1 })
}

describe('hooked', () => {
  setup()
  test('reads', () => { expect(state.value).toBe(1) })
})

describe('plain', () => {
  test('reads', () => { expect(state.value).toBe(1) })
})
