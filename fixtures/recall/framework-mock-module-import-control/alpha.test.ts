// RECALL control -- duplicate-test, mock module imported for its effects only.
//
// The same shape as framework-mock-module-effects except the file only imports the mock module and
// never calls anything from it. That import is accepted today, so the identical pair in each file
// is reported. It fails a fix that starts refusing every import of a module with setup effects.
// The two files import different mock modules (a different user), and the tests are identical;
// today the cross-file rule still pairs them because the imported module's mock is not part of the
// compared context. Add a third statement to one body and the in-file pair differs.
import { expect, test } from 'vitest'
import './mocks-alpha'
import { fetchUser } from './api'

test('alpha: loads the user', () => {
  expect(typeof fetchUser().id).toBe('number')
})

test('alpha: loads the user again', () => {
  expect(typeof fetchUser().id).toBe('number')
})
