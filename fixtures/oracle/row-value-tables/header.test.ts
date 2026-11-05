import { expect, test } from 'vitest'

test.each`x | \ty
${1} | ${2}`('tabbed header', ({ y }) => expect(y).toBeUndefined())
test('literal', () => expect(2).toBeUndefined())
