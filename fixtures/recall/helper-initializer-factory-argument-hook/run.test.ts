// RECALL control -- duplicate-test absent. A factory argument runs a hook; an unwalked argument must make its scope incomplete.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
beforeEach(() => { state.value = 0 })
const setup = () => {
  const factory = test.skipIf(beforeEach(() => { state.value = 1 })).each([1])
}

describe('hooked', () => {
  setup()
  test('reads', () => { expect(state.value).toBe(1) })
})

describe('plain', () => {
  test('reads', () => { expect(state.value).toBe(1) })
})
