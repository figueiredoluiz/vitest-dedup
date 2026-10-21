// RECALL positive -- duplicate-test, shape: called helper from a mock module with setup effects.
//
// `useMocks()` comes from a module whose import runs `vi.hoisted` and `vi.mock` at module level,
// and the call has an empty body. The two tests below are identical and observe the same mock, so
// a duplicate-test is wanted once such a module is modelled. Today the call is refused and the file
// is set aside (not-compared). The sibling file uses a module whose factory returns a different
// user, so a pair across the two files must not be reported. Add a third statement to one body and
// the pair differs.
import { expect, test } from 'vitest'
import { useMocks } from './mocks-beta'
import { fetchUser } from './api'

useMocks()

test('beta: loads the user', () => {
  expect(typeof fetchUser().id).toBe('number')
})

test('beta: loads the user again', () => {
  expect(typeof fetchUser().id).toBe('number')
})
