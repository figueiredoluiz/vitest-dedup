// RECALL positive -- duplicate-test, shape: the runner module creates module-local values
// (`vi.fn()` inside an object, `new Set()`, an array, a class with a literal static field).
//
// Creating a value changes nothing the importing file can observe except through the module's own
// bindings, so the runner registers exactly what its body says and its test is compared with the
// hand-written one below. Treating value creation as a setup effect sets the whole file aside as not
// compared. Make the runner's body differ and the finding disappears.
import { expect, test } from 'vitest'
import { defineSlugChecks } from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
