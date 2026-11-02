import { beforeEach, expect, test } from 'vitest'

const state = { n: 0 }
beforeEach(() => { state.n = 0 })
const observed = {
  get extra() { state.n++; return 0 },
  get width() { return state.n },
}

test('binding member pass', () => {
  expect(2).toBe(2)
  expect(3).toBe(3)
  const { width } = observed
  expect(width).toBe(0)
})
test('binding member fail', () => {
  expect(2).toBe(2)
  expect(3).toBe(3)
  const { extra, width } = observed
  expect(width).toBe(0)
})

test('binding unused last pass', () => {
  const { width } = observed
  expect([width, state.n]).toEqual([0, 0])
  expect(12).toBe(12)
})
test('binding unused last fail', () => {
  const { width, extra } = observed
  expect([width, state.n]).toEqual([0, 0])
  expect(12).toBe(12)
})

function source() {
  return { width: 1, get extra() { state.n++; return 0 } }
}
test('binding observer pass', () => {
  const { width } = source()
  expect(state.n).toBe(0)
  expect(state.n).toBeLessThan(1)
})
test('binding observer fail', () => {
  const { width, extra } = source()
  expect(state.n).toBe(0)
  expect(state.n).toBeLessThan(1)
})

function scalar() {
  let width = 1
  return { get width() { return width }, get extra() { width = 2; return 0 } }
}
test('binding scalar pass', () => {
  const { width } = scalar()
  expect(width).toBe(1)
  expect(width).toBeGreaterThan(0)
})
test('binding scalar fail', () => {
  const { extra, width } = scalar()
  expect(width).toBe(1)
  expect(width).toBeGreaterThan(0)
})

function aliased() {
  const width = { value: 1 }
  return { width, get extra() { width.value = 2; return 0 } }
}
test('binding alias pass', () => {
  const { width } = aliased()
  expect(width.value).toBe(1)
  expect(width.value).toBeGreaterThan(0)
})
test('binding alias fail', () => {
  const { width, extra } = aliased()
  expect(width.value).toBe(1)
  expect(width.value).toBeGreaterThan(0)
})

function plain() { return { width: 1, extra: 2 } }
test('binding plain smaller', () => {
  const { width } = plain()
  expect(width).toBe(1)
  expect(width).toBeGreaterThan(0)
})
test('binding plain larger', () => {
  const { width, extra } = plain()
  expect(width).toBe(1)
  expect(width).toBeGreaterThan(0)
  expect(extra).toBe(2)
})

const stable = { width: 7, extra: 3 }
test('binding retained member', () => {
  const { width } = stable
  expect(width).toBe(7)
  expect(2).toBe(2)
})
test('binding written member', () => {
  expect(stable.width).toBe(7)
  expect(2).toBe(2)
})
test('binding reordered one', () => {
  const { width, extra } = stable
  expect(4).toBe(4)
  expect(5).toBeGreaterThan(0)
})
test('binding reordered two', () => {
  const { width: renamed, extra: unused } = stable
  expect(5).toBeGreaterThan(0)
  expect(4).toBe(4)
})
test('binding ordinary smaller', () => { expect(8).toBe(8) })
test('binding bookkeeping larger', () => {
  const { width, extra } = stable
  expect(8).toBe(8)
})
test('binding ordinary larger', () => {
  const { width, extra } = stable
  expect(8).toBe(8)
  expect(9).toBe(9)
})
test('binding only one', () => { const { width = 1 } = stable })
test('binding only two', () => { const { width = 2 } = stable })

const incrementing = { get value() { return ++state.n } }
test('binding repeated pass', () => {
  const { value } = incrementing
  expect([value, value]).toEqual([1, 1])
  expect(6).toBe(6)
})
test('binding repeated fail', () => {
  expect([incrementing.value, incrementing.value]).toEqual([1, 1])
  expect(6).toBe(6)
})

test('binding movement pass', () => {
  const before = state.n
  const { extra } = observed
  expect(before).toBe(0)
  expect(10).toBe(10)
})
test('binding movement fail', () => {
  const { extra } = observed
  expect(state.n).toBe(0)
  expect(10).toBe(10)
})

function* values() { yield 0; state.n++; yield 1 }
test('binding iterator pass', () => {
  const [first] = values()
  expect(state.n).toBe(0)
  expect(11).toBe(11)
})
test('binding iterator fail', () => {
  const [first, second] = values()
  expect(state.n).toBe(0)
  expect(11).toBe(11)
})
