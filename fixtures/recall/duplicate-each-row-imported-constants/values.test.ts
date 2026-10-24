// RECALL positive -- duplicate-each-row over cells that name imported `const` scalars.
//
// Each imported name holds the same value as the literal row beside it, so each pair renders
// one title and binds one value: through a direct import, an import alias, a renamed local
// export, a local alias in the declaring module, and a named re-export.
import { expect, test } from 'vitest'
import { ALIAS, BIG, COUNT, MODE, NONE, ON, RENAMED as R, TEMPLATE } from './values'
import { BARREL_SPEED } from './barrel'

test.each([
  [MODE], ['fast'],
  [BARREL_SPEED], ['quick'],
  [COUNT], [-2],
  [BIG], [10n],
  [ON], [true],
  [NONE], [null],
  [TEMPLATE], ['tpl'],
  [ALIAS], [R],
])('reads %s', (value) => {
  expect(value).toBeDefined()
})

test.each`
  value
  ${MODE}
  ${'fast'}
`('tags $value', ({ value }) => {
  expect(value).toBeTruthy()
})
