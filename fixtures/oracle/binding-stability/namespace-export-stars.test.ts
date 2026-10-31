// The first of several stars wins, and it holds the name without registering.
import { expect, test } from 'vitest'
import { alias } from './namespace-export-stars'

alias.test('never registered', () => {})
test('stays comparable', () => {
  expect(1).toBe(1)
})
