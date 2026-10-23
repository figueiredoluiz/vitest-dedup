// RECALL positive -- duplicate-test, shape: top-level call to an installed package that defines
// and calls its own function named `test`, under `globals: true`.
//
// Retention control for framework-package-parenthesised-registration: `test` is bound in the
// package, so it is not the ambient global, the call is setup, and the identical pair is reported.
// A text scan flags every `test(` and leaves the pair not-compared.
import { installSuite } from '@test-kit/suite'

installSuite()

test('lists users: first check', () => {
  expect(1 + 1).toBe(2)
})

test('lists users: second check', () => {
  expect(1 + 1).toBe(2)
})
