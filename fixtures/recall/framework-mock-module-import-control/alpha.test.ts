// RECALL control -- duplicate-test, mock module imported for its effects only.
//
// The same shape as framework-mock-module-effects except the file only imports the mock module and
// never calls anything from it. That import is accepted today, so the identical pair in each file
// is reported. It fails a fix that starts refusing every import of a module with setup effects.
// The two files import different mock modules (a different user), and the tests are identical;
// the cross-file rule keeps them apart because the imported mocks differ in the module context.
// The same-file pair is the positive control. Add a third statement to one body and it differs.
import { expect, test } from 'vitest'
import './mocks-alpha'
import { fetchUser } from './api'

test('alpha: loads the user', () => {
  expect(typeof fetchUser().id).toBe('number')
})

test('alpha: loads the user again', () => {
  expect(typeof fetchUser().id).toBe('number')
})
