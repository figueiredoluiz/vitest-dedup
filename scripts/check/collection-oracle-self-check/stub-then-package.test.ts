// Reads a missing module's `then` through a package under the root's own node_modules.
import { test } from 'vitest'
import { enabled } from 'stub-package'

if (enabled) test('only when enabled', () => {})
test('always', () => {})
