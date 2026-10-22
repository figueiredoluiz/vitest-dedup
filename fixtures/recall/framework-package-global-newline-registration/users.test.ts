// RECALL variant -- duplicate-test, shape: top-level call to an installed package that registers a
// test through an ambient global written across two lines (`test` newline `(`).
//
// Under `globals: true` the package registers a test for the calling file, so the call is not
// setup and the pair must stay not-compared. Join the call onto one line and the package is already
// refused today; the control twin keeps a package that only has a `test` method.
import { installSuite } from '@test-kit/suite'

installSuite()

test('lists users: first check', () => {
  expect(1 + 1).toBe(2)
})

test('lists users: second check', () => {
  expect(1 + 1).toBe(2)
})
