// An export named `then` cannot be told from the runtime awaiting the import, so it counts.
import { test } from 'vitest'
import { then as enabled } from './missing-module'

if (enabled) test('only when enabled', () => {})
test('always', () => {})
