import { expect, test } from 'vitest'
import { and, or, coalesced } from './values'
and('and left', () => expect(1).toBe(1))
and('and right', () => expect(1).toBe(1))
or('or left', () => expect(1).toBe(1))
or('or right', () => expect(1).toBe(1))
coalesced('nullish left', () => expect(1).toBe(1))
coalesced('nullish right', () => expect(1).toBe(1))
test('direct left', () => expect(2).toBe(2))
test('direct right', () => expect(2).toBe(2))
