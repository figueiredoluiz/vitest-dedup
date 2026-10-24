// RECALL negative -- an import cycle writes the member before the export reads it: Vitest titles
// the rows `cycle y` and `cycle x`. Reading `LEVELS.a` from its initializer would report a duplicate.
import { expect, test } from 'vitest'
import { X } from './cycle'

test.each([[X], ['x']])('cycle %s', (value) => {
  expect(value).toBeDefined()
})
