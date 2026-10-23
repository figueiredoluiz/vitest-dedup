// RECALL variant -- duplicate-test, shape: top-level call to an installed package that registers a
// test through an ambient global written in parentheses, `(test)(...)`.
//
// Under `globals: true` the package registers a test for the calling file, so the call is not setup
// and the pair must stay not-compared. The package scan reads the syntax tree: a free `test` is
// the global however it is written. A text scan that looks for `test(` misses the parentheses and
// reports the pair.
import { installSuite } from '@test-kit/suite'

installSuite()

test('lists users: first check', () => {
  expect(1 + 1).toBe(2)
})

test('lists users: second check', () => {
  expect(1 + 1).toBe(2)
})
