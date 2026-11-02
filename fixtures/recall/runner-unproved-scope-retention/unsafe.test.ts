// Unproved runner effects qualify the entire calling scope, including earlier declarations.
import { test, expect } from 'vitest'
import { setup } from './helper'

test('uncertain first', () => {
  expect(' Label '.trim().toLowerCase()).toBe('label')
})
setup()
test('uncertain second', () => {
  expect(' Label '.trim().toLowerCase()).toBe('label')
})
