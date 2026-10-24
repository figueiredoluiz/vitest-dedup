// Imports a missing module only for what it does when it loads, which a stub cannot reproduce.
import { test } from 'vitest'
import './missing-module'

test('always', () => {})
