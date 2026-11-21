import { expect, test } from 'vitest'
import conditional, { and, or, coalesced } from './values'
and('and left', () => expect(1).toBe(1))
and('and right', () => expect(1).toBe(1))
or('or left', () => expect(1).toBe(1))
or('or right', () => expect(1).toBe(1))
coalesced('nullish left', () => expect(1).toBe(1))
coalesced('nullish right', () => expect(1).toBe(1))
conditional('conditional left', () => expect(1).toBe(1))
conditional('conditional right', () => expect(1).toBe(1))
;(true && import.meta.vitest.test)('in-source left', () => expect(1).toBe(1))
;(false || import.meta.vitest.test)('in-source right', () => expect(1).toBe(1))
test('direct left', () => expect(2).toBe(2))
test('direct right', () => expect(2).toBe(2))
