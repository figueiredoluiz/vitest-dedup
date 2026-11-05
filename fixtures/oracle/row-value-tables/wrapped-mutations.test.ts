import { expect, test } from 'vitest'

const deleted = { n: 1 }
delete (deleted.n)
test.each([[deleted.n], [1]])('parenthesized delete', value => expect(value).toBe(1))

const erased = { n: 1 }
delete (erased.n as number)
test.each([[erased.n], [1]])('erased delete', value => expect(value).toBe(1))

const updated = { n: 1 }
;(updated.n)++
test.each([[updated.n], [1]])('parenthesized update', value => expect(value).toBe(1))

const typed = { n: 1 }
;(typed.n as number)++
test.each([[typed.n], [1]])('erased update', value => expect(value).toBe(1))

const stable = { n: 1 }
test.each([[stable.n], [1]])('stable control', value => expect(value).toBe(1))
