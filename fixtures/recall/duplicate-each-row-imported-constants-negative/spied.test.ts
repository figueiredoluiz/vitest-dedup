// RECALL negative -- a namespace spy replaces the export's getter before the table is built:
// Vitest titles the rows `spied slow` and `spied fast`. One edit away from firing: drop the spy.
import { expect, test, vi } from 'vitest'
import * as spied from './spied'
import { MODE } from './spied'

vi.spyOn(spied, 'MODE', 'get').mockReturnValue('slow' as never)

test.each([[MODE], ['fast']])('spied %s', (value) => {
  expect(value).toBeDefined()
})
