import { test, expect } from 'vitest'

test.each`value
${1}`('tagged lexical this first', () => expect(this).toBe(this))
test.each`value
${2}`('tagged lexical this second', () => expect(this).toBe(this))
test.for`value
${3}`('tagged lexical arguments first', () => expect(typeof arguments).toBe(typeof arguments))
test.for`value
${4}`('tagged lexical arguments second', () => expect(typeof arguments).toBe(typeof arguments))
test.each`value
${5}`('tagged control first', () => expect('control').toBe('control'))
test.each`value
${6}`('tagged control second', () => expect('control').toBe('control'))
