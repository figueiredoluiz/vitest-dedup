import { test } from 'vitest'
import { inner } from './b'

/** Calls into the cycle. */
export const outer = () => inner()

/** Registers a test. */
export const leaf = () => test('registered through the cycle', () => {})
