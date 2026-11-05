import { expect, test } from 'vitest'

test.each`a | b ${1} | ${2}
${1} | ${2}`('inline header', ({ a, b }) => expect(a + b).toBe(3))

test.each`
a | b ${1} | ${2}
${1} | ${2}`('leading newline', ({ a, b }) => expect(a + b).toBe(3))

test.each`a | b
${1} | ${2}
${1} | ${2}`('terminated header', ({ a, b }) => expect(a + b).toBe(3))
