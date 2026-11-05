import { expect, test } from 'vitest'

let counter = 0
test.each`value
${counter++}`('first', ({ value }) => expect(value).toBe(0))
test.each`value
${counter++}`('second', ({ value }) => expect(value).toBe(0))
