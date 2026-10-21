// RECALL positive -- duplicate-test, control for the default-export runner cases.
//
// The identical source with a NAMED export is expanded today, so the runner's test and the
// hand-written one are compared and reported. If the default-export cases start finding their pair
// and this one stops, a fix for default exports broke named ones. Make the runner's body differ
// from the hand-written test and the finding disappears.
import { expect, test } from 'vitest'
import { defineSlugChecks } from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
