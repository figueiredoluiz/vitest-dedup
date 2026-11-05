import { expect, test } from 'vitest'

const { n } = { n: 'abc', length: 7 }
test.each([[n.length], [7]])('destructured', value => expect(value).toBe(3))

const { n: alias } = { n: 'abc', length: 7 }
test.each([[alias.length], [7]])('renamed', value => expect(value).toBe(3))

const { n: defaulted = 'fallback' } = { n: 'abc', length: 7 }
test.each([[defaulted.length], [7]])('defaulted', value => expect(value).toBe(3))

const plain = { n: 'abc', length: 7 }
test.each([[plain.length], [7]])('plain control', value => expect(value).toBe(7))
