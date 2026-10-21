// RECALL variant -- duplicate-test, shape: top-level call to a package's mocking API, package
// absent.
//
// The same source as framework-package-mock-call without `node_modules/@test-kit/runtime`. With
// the package missing the calls cannot be known to register mocks, so the pair must stay
// not-compared after the installed case is understood, and the run should name the missing package
// as the cause. Install the package and it becomes the positive case.
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
