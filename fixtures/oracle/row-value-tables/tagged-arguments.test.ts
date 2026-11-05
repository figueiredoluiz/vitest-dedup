import { expect, test } from 'vitest'

test.each`value
${1}`('arguments one', function () { expect(arguments[0].value).toBe(1) })
test.each`value
${2}`('arguments two', function () { expect(arguments[0].value).toBe(1) })

test.each`value
${1}`('destructured one', function ({ value }) { expect(arguments[0].value).toBe(1) })
test.each`value
${2}`('destructured two', function ({ value }) { expect(arguments[0].value).toBe(1) })

test.each`value
${1}`('arrow control one', () => expect(1).toBe(1))
test.each`value
${2}`('arrow control two', () => expect(1).toBe(1))
