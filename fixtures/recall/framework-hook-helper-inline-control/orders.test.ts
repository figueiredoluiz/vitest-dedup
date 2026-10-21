// RECALL positive -- duplicate-test, control for framework-hook-helper-server.
//
// The same hooks registered through inline callbacks with no module state are read today, so the
// identical pair is compared and reported. It fails a fix that refuses every hook-registering
// helper. Add a third statement to one body and the pair differs.
import { expect, test } from 'vitest'
import { setupServer } from './server'

setupServer()

test('orders: first check', () => {
  expect(['a', 'b'].join('-')).toBe('a-b')
})

test('orders: second check', () => {
  expect(['a', 'b'].join('-')).toBe('a-b')
})
