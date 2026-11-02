import { expect, test } from 'vitest'

// Erased table wrappers preserve rows, callback inputs and duplicate-row claims.
test.each([[1], [1], [2]])('plain %i', value => expect(value).toBeGreaterThan(0))
test.each([[3], [3], [4]] as const)('const %i', value => expect(value).toBeGreaterThan(0))
test.each([[5], [5], [6]] satisfies number[][])('satisfies %i', value => expect(value).toBeGreaterThan(0))
test.each(<number[][]>[[7], [7], [8]])('asserted %i', value => expect(value).toBeGreaterThan(0))
test.each(([[9], [9], [10]])!)('nonnull %i', value => expect(value).toBeGreaterThan(0))
test.each((([[11], [11], [12]] as const) satisfies readonly (readonly number[])[])!)('nested %i', value => expect(value).toBeGreaterThan(0))
test.for(([[13], [13], [14]] as const) satisfies readonly (readonly number[])[])('whole %i', ([value]) => expect(value).toBeGreaterThan(0))

// Outer holes register nothing; the original array index still formats each surviving title.
test.each([15, , 15, 16])('sparse scalar %i', value => expect(value).toBeGreaterThan(0))
test.each([, [17], , [17], [18], ,] as const)('sparse spread %i', value => expect(value).toBeGreaterThan(0))
test.for([, [19], , [19], [20], ,] as const)('sparse whole %i', ([value]) => expect(value).toBeGreaterThan(0))
test.each([, [21], , [21], ,] as const)('each index %# position %$', value => expect(value).toBe(21))
test.for([, [22], , [22], ,] as const)('for index %# position %$', ([value]) => expect(value).toBe(22))
test.each([, [23], 23])('mixed %#', value => expect(value).toBeDefined())
test.each([undefined, , undefined])('explicit undefined %#', value => expect(value).toBeUndefined())

// Empty tables stay empty; excluded callbacks still retain the surviving row indexes.
test.each([])('empty', () => expect.unreachable())
test.each([, ,] as const)('only holes', () => expect.unreachable())
test.for([, ,] satisfies unknown[])('whole holes', () => expect.unreachable())
test.each([, [1], , [1]] as const)('empty callback %#', () => {})
