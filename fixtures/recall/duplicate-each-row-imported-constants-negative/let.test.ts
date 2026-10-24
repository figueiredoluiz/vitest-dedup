// RECALL negative -- an imported `let` is reassigned before the table is built: Vitest titles the
// rows `let after` and `let before`. Reading the initializer would report a duplicate.
import { expect, test } from 'vitest'
import { LET, setLet } from './values'

setLet()

test.each([[LET], ['before']])('let %s', (value) => {
  expect(value).toBeDefined()
})
