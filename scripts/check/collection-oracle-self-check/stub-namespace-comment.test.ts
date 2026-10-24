// The same namespace import with a comment between its tokens: detection reads syntax, not text.
import { test } from 'vitest'
import * /* namespace */ as flags from './missing-module'

if ('enabled' in flags) test('only when enabled', () => {})
test('always', () => {})
