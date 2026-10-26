// The same check without the runner: `./slug` is real here.
import { expect, test } from 'vitest'
import { slug } from './slug'

test('page slug is lowercase', () => {
  expect(slug('Home-Page')).toBe('home-page')
})
