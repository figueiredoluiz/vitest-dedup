// RECALL positive -- duplicate-test, shape: helper that registers beforeAll/afterAll hooks.
//
// `setupServer()` registers a server start and stop for the file. Both tests run under the same
// hooks and are identical, so a duplicate-test is wanted once the helper's hooks are read as
// context. Today the helper is refused (module state plus member calls in the hooks) and the file
// is set aside (not-compared). Add a third statement to one body and the pair differs.
import { expect, test } from 'vitest'
import { setupServer } from './server'

setupServer()

test('orders: first check', () => {
  expect(['a', 'b'].join('-')).toBe('a-b')
})

test('orders: second check', () => {
  expect(['a', 'b'].join('-')).toBe('a-b')
})
