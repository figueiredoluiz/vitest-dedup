import { describe, test, expect, beforeEach } from 'vitest'

const enabled = true
const state = { n: 0 }

function registerSuite() {
  beforeEach(() => { state.n = 1 })
  test('reads', () => { expect(state.n).toBe(1) })
}

function registerOtherSuite() {
  beforeEach(() => { state.n = 1 })
  test('reads', () => { expect(state.n).toBe(1) })
}

if (enabled) describe('suite-a', () => registerSuite())
if (enabled) describe('suite-b', () => { registerOtherSuite() })
