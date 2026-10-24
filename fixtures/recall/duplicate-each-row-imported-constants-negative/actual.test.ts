// RECALL negative -- `vi.importActual` returns the module instance the import binds, so spying its
// getter rewrites MODE: Vitest titles the rows `actual slow` and `actual fast`.
import { expect, test, vi } from 'vitest'
import { MODE } from './actual'

const actual = await vi.importActual<typeof import('./actual')>('./actual')
vi.spyOn(actual, 'MODE', 'get').mockReturnValue('slow' as never)

test.each([[MODE], ['fast']])('actual %s', (value) => {
  expect(value).toBeDefined()
})
