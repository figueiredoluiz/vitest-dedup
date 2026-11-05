import { expect, test } from 'vitest'

const box = { n: 1 }
box.__defineGetter__('n', () => 2)
test.each([[box.n], [1]])('member', n => expect(n).toBeDefined())
