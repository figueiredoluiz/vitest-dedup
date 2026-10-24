// Takes a missing module whole: `in` and `Object.keys` read no name, so the import itself counts.
import { test } from 'vitest'
import * as flags from './missing-module'

if ('enabled' in flags) test('only when enabled', () => {})
test('always', () => {})
