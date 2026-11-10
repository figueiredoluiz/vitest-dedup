import { describe, expect, test } from 'vitest'

describe('sibling', () => {
  let written = 'before'
  describe('first', () => {
    written = 'from first'
  })
  describe('second', () => {
    test(written, () => { expect(written).toBe('from first') })
  })
})

describe.each([['stop'], ['go']])('%s', (key) => {
  let stopped = 'before'
  describe('inner', () => {
    test(stopped, () => { expect(stopped.length).toBeGreaterThan(0) })
  })
  if (key === 'stop') return
  stopped = 'after'
})

describe('awaited', async () => {
  let pending = 'before'
  describe('inner', () => {
    test(pending, () => { expect(pending).toBe('after await') })
  })
  await Promise.resolve()
  pending = 'after await'
})

describe('mapped', () => {
  let mapped = 'before'
  const rows = ['row'].map((row) => `${mapped} ${row}`)
  mapped = 'after'
  test.each(rows)('%s', (row) => { expect(row).toContain('row') })
})

describe('looped', () => {
  let each = 'before'
  ;['row'].forEach((row) => {
    test(each, () => { expect(row).toBe('row') })
  })
  each = 'after'
})
