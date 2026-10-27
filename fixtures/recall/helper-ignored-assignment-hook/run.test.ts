// RECALL control -- duplicate-test absent. An unwalked assignment may register a hook, so the affected test is not compared.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
beforeEach(() => { state.value = 0 })
const setup = () => {
  let stop; stop = beforeEach(() => { state.value = 1 })
}

describe('hooked', () => {
  setup()
  test('reads', () => { expect(state.value).toBe(1) })
})

describe('plain', () => {
  test('reads', () => { expect(state.value).toBe(1) })
})
