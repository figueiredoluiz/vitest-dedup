import { beforeEach, describe, expect, test, vi } from 'vitest'

let value = 0
describe('constant overwrite', () => {
  beforeEach(() => { value = 1 })
  beforeEach(() => { value = 1 })
  test('keeps the same value', () => { expect(value).toBe(1) })
})

describe('framework reset', () => {
  beforeEach(() => { vi.clearAllMocks() })
  beforeEach(() => { vi.clearAllMocks() })
  test('still runs', () => { expect(2 + 3).toBe(5) })
})

let calls = 0
let handle: unknown
function connect() { calls += 1; return undefined }
describe('falsy initialization', () => {
  beforeEach(() => { if (!handle) handle = connect() })
  beforeEach(() => { if (!handle) handle = connect() })
  test('needs both calls', () => { expect(calls).toBe(2) })
})

let writes = 0
const state = { set value(_value: number) { writes += 1 } }
describe('setter effects', () => {
  beforeEach(() => { state.value = 1 })
  beforeEach(() => { state.value = 1 })
  test('needs both setters', () => { expect(writes).toBe(2) })
})

let observed = 0
let barrierValue = 0
describe('intervening work', () => {
  beforeEach(() => { barrierValue = 1 })
  beforeEach(() => { observed = barrierValue })
  beforeEach(() => { barrierValue = 1 })
  test('observes the first overwrite', () => { expect([barrierValue, observed]).toEqual([1, 1]) })
})
