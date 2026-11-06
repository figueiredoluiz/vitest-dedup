import { expect, test } from 'vitest'

test('tag \uDB40\u200B\uDC61', () => { expect(7).toBe(7) })
test('nested \uDB40\uDB40\uDC61\uDC61', () => { expect(7).toBe(7) })
test('emoji \uD83D\u200B\uDE00', () => { expect(7).toBe(7) })
test('lookalike \uFFFDdb40\u200B\uFFFDdc61', () => { expect(7).toBe(7) })
