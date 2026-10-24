// Collection reads an export of a module that does not exist: the gate must not compare this file.
import { describe, test } from 'vitest'
import { enabled } from './missing-module'

describe('reads a stub while collecting', () => {
  if (enabled) test('only when enabled', () => {})
  test('always', () => {})
})
