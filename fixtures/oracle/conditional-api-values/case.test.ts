import { expect, test } from 'vitest'
import byDefault from './default'
import { either, extended, first, listed, member, neither, nested, none, nullish, ternary } from './values'
ternary('ternary', () => expect(1).toBe(1))
first('first', () => expect(1).toBe(1))
either('either', () => expect(1).toBe(1))
nullish('nullish', () => expect(1).toBe(1))
extended('extended', () => expect(1).toBe(1))
nested('nested', () => expect(1).toBe(1))
listed('listed', () => expect(1).toBe(1))
member('member', () => expect(1).toBe(1))
byDefault('default', () => expect(1).toBe(1))
neither('neither', () => expect(1).toBe(1))
none('none', () => expect(1).toBe(1))
test('inline', () => expect(1).toBe(1))
