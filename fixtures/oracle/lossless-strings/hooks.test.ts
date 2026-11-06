import { beforeEach, describe, expect, test } from 'vitest'

describe('equal primitive writes', () => {
  let value = ''
  beforeEach(() => { value = '\uD800' })
  beforeEach(() => { value = '\uD800' })
  test('keeps the surrogate', () => { expect(value).toBe('\uD800') })
})
describe('conflicting primitive writes', () => {
  let value = ''
  beforeEach(() => { value = '\uD800' })
  beforeEach(() => { value = '\uFFFDd800' })
  test('keeps the replacement text', () => { expect(value).toBe('\uFFFDd800') })
})
