import { describe, expect, test } from 'vitest'

describe.each([{ scope: ['read'] }, { scope: ['read', 'write'] }])('scope', ({ scope }) => {
  test('first', () => expect(scope.length).toBeGreaterThan(0))
  test('second', () => expect(scope.length).toBeGreaterThan(0))
})
