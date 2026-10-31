// A namespace alias is the namespace across an export, under each spelling here.
import byDefault, { alias, renamed } from './namespace-exports'
import { viaBarrel } from './namespace-exports-barrel'

alias.beforeEach(() => {})
alias.test('through an alias', () => {
  alias.expect(1).toBe(1)
})
renamed.test('through a rename', () => {
  renamed.expect(2).toBe(2)
})
byDefault.test('through the default export', () => {
  byDefault.expect(3).toBe(3)
})
viaBarrel.test('through a barrel', () => {
  viaBarrel.expect(4).toBe(4)
})
