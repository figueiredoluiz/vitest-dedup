// RECALL negative -- cross-file-duplicate, shape: package mocking call with a different factory.
//
// This file and its sibling declare identical test bodies, but each `mockImport` factory returns
// a different value, so the tests observe different clocks. No finding is wanted before or after
// the mocking API is understood. It fails a fix that marks the package's mocking calls inert (the
// factories would vanish from the context and the pair would be reported). Give both files the
// same factory and it becomes a cross-file-duplicate.
import { expect, test } from 'vitest'
import { mockImport } from '@test-kit/runtime'

mockImport('useClock', () => () => 2)

test('reads the clock (evening)', () => {
  expect(useClock()).toBeGreaterThan(0)
})
