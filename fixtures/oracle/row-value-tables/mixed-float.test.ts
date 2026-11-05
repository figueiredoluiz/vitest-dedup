import { expect, test } from 'vitest'

test.each([[1, -0]])('mixed %s %f', (a, b) => expect([a, b]).toHaveLength(2))
test('mixed 1 -0', () => expect(0).toBe(0))
test.each([[-0, 1]])('prefix %s %f', (a, b) => expect([a, b]).toHaveLength(2))
test.each([[-0, -0, -0]])('floats %f %s %f', (a, b, c) => expect([a, b, c]).toHaveLength(3))
