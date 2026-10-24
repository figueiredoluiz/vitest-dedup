// Takes a missing module whole; with isolate: false the second file shares the first's load.
import { test } from 'vitest'
import * as flags from './missing-module'

if ('enabled' in flags) test('only when enabled', () => {})
test('second', () => {})
