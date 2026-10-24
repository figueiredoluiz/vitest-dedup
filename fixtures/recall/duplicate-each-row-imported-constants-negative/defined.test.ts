// RECALL negative -- a namespace re-exported by another module has its getter redefined before
// the table is built: Vitest titles the rows `defined slow` and `defined fast`.
import { expect, test } from 'vitest'
import { defined } from './defined-barrel'
import { MODE } from './defined'

Object.defineProperty(defined, 'MODE', { get: () => 'slow' })

test.each([[MODE], ['fast']])('defined %s', (value) => {
  expect(typeof value).toBe('string')
})
