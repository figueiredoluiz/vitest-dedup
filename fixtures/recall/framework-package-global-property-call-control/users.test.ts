// RECALL positive -- duplicate-test, shape: top-level call to an installed package that has a
// method named `test` but registers nothing, under `globals: true`.
//
// Retention control for framework-package-global-newline-registration: a property call
// (`patterns.test(...)`) is not the ambient global, so the call is setup and the identical pair is
// reported.
import { installSuite } from '@test-kit/suite'

installSuite()

test('lists users: first check', () => {
  expect(1 + 1).toBe(2)
})

test('lists users: second check', () => {
  expect(1 + 1).toBe(2)
})
