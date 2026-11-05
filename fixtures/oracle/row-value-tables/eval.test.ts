import { expect, test } from 'vitest'
import { VALUE } from './eval-values'

test.each([[VALUE], [1]])('value', n => expect(n).toBeGreaterThan(0))
