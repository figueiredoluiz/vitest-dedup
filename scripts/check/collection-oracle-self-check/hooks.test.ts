// Each hook below is registered, or not, in a way the probe must report exactly.
import { afterEach, beforeEach, describe, test } from 'vitest'
import { useReset } from './hook-helper'

describe('conditional', () => {
  if (false) beforeEach(() => {})
  test('a', () => {})
})

describe('loop', () => {
  for (let i = 0; i < 2; i++) afterEach(() => {})
  test('b', () => {})
})

describe('member call', () => {
  test.beforeEach(() => {})
  test('c', () => {})
})

describe('after return', () => {
  test('d', () => {})
  return
  beforeEach(() => {})
})

describe('helper', () => {
  useReset()
  test('e', () => {})
})
