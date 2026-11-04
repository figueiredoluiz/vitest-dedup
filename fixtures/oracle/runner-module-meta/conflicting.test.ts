// The runner passes and this written-out lookalike fails: their metadata names different modules.
import { expect, test } from 'vitest'
import { run } from './runner'

run('imported')

test('direct', () => {
  expect(import.meta.url.endsWith('/runner.ts')).toBe(true)
})
