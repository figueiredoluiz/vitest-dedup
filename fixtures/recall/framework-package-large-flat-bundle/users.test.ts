// RECALL positive -- duplicate-test, shape: top-level call to an installed package that is one
// large flat bundle, under `globals: true`.
//
// The package is 9,000 statements long but shallow and registers nothing, so the call is setup and
// the identical pair is reported. A package scan that refuses large files by size leaves the pair
// not-compared and the finding disappears.
import { install } from '@test-kit/bundle'

install()

test('lists users: first check', () => {
  expect(1 + 1).toBe(2)
})

test('lists users: second check', () => {
  expect(1 + 1).toBe(2)
})
