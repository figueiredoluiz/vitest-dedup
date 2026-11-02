// An independent file retains a meaningful exact-duplicate positive.
import { test, expect } from 'vitest'

test('safe first', () => {
  expect(' Label '.trim().toLowerCase()).toBe('label')
})
test('safe second', () => {
  expect(' Label '.trim().toLowerCase()).toBe('label')
})
