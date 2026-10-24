// Reads a missing module's `then` through a helper's re-export.
import { test } from 'vitest'
import { enabled } from './then-helper'

if (enabled) test('only when enabled', () => {})
test('always', () => {})
