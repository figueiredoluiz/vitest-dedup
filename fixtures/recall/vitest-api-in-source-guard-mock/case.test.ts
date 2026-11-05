// RECALL positive -- duplicate-test expected. Vitest hoists `vi.mock` from the top level of an
// `if (import.meta.vitest)` block as it does from the module's, so the file runs and its two
// tests are equal. Moving the mock into a test makes Vitest refuse the file.
import { expect, vi } from 'vitest'
import { value } from './dep'

if (import.meta.vitest) {
  const { test } = import.meta.vitest
  vi.mock('./dep', () => ({ value: 'mocked' }))
  test('reads the mock', () => expect(value).toBe('mocked'))
  test('reads the mock, again', () => expect(value).toBe('mocked'))
}
