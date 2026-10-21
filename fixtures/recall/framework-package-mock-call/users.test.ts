// RECALL positive -- duplicate-test, shape: top-level call to an installed package's mocking API.
//
// `mockImport(name, factory)` and `registerEndpoint(url, handler)` take a static name and an inline
// factory, so each is a mock declaration with a known effect and the same for both tests below. The
// identical pair is wanted once such calls are understood. Today the called package import is
// refused and the file is set aside (not-compared). Add a third statement to one body and the pair
// differs.
import { expect, test } from 'vitest'
import { mockImport, registerEndpoint } from '@test-kit/runtime'

mockImport('useClock', () => () => 1_700_000_000)
registerEndpoint('/api/users', () => ({ users: [] }))

test('lists users: first check', async () => {
  const response = await fetch('/api/users')
  expect(await response.json()).toEqual({ users: [] })
})

test('lists users: second check', async () => {
  const response = await fetch('/api/users')
  expect(await response.json()).toEqual({ users: [] })
})
