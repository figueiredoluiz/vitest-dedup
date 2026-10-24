// RECALL positive -- duplicate-each-row over cells that name imported `const` scalars.
//
// Each imported name holds the same value as the literal row beside it, so each pair renders
// one title and binds one value: through a direct import, an import alias, a renamed local
// export, a local alias in the declaring module, a named re-export, and an import exported again.
import { expect, test } from 'vitest'
import { ALIAS, BIG, COUNT, MODE, NONE, ON, RENAMED as R, TEMPLATE } from './values'
import { BARREL_OFF, BARREL_SPEED } from './barrel'

test.each([
  [MODE], ['fast'],
  [BARREL_SPEED], ['quick'],
  [COUNT], [-2],
  [BIG], [10n],
  [ON], [true],
  [BARREL_OFF], [false],
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
