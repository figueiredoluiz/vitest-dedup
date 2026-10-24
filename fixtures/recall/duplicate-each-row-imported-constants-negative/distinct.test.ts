// RECALL negative -- two imported constants with different values never compare equal.
// One edit away from firing: make SLOW 'fast' in values.ts.
import { expect, test } from 'vitest'
import { FAST, SLOW } from './values'

test.each([[FAST], [SLOW]])('runs %s', (mode) => {
  expect(mode).toBeDefined()
})
