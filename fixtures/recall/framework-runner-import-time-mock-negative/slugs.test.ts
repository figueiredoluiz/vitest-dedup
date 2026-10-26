// RECALL negative -- cross-file-duplicate, shape: a module the runner imports calls `vi.mock` while
// it loads.
//
// The mock reaches every test in this file, not only the runner's: both tests here see `mocked` and
// fail, while the same test in `other.test.ts` passes. No test's context records the mock, so this
// file stays set aside as not compared and is never paired with `other.test.ts`. A change that
// admits `vi` calls in a module imported at load would report that pair. Remove the `vi.mock` call
// and the runner is read again.
import { expect, test } from 'vitest'
import { defineSlugChecks } from './runner'
import { slug } from './slug'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  expect(slug('Home-Page')).toBe('home-page')
})
