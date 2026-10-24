// RECALL negative -- deleting the export's getter from its module object rewrites MODE to
// `undefined`: Vitest titles the rows `deleted undefined` and `deleted fast`.
import { expect, test } from 'vitest'
import * as deleted from './deleted'
import { MODE } from './deleted'

delete (deleted as Record<string, unknown>).MODE

test.each([[MODE], ['fast']])('deleted %s', (value) => {
  expect([undefined, 'fast']).toContain(value)
})
