// Uses a value a helper cached from a stub; with isolate: false the later file reuses the cached helper.
import { test } from 'vitest'
import { flag } from './named-helper'

if (flag) test('only when enabled', () => {})
test('third', () => {})
