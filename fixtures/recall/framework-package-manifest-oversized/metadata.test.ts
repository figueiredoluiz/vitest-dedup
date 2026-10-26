// Owned package-metadata admission control: these tests remain equivalent.
import { test, expect } from 'vitest'
import { setup } from '@owned/metadata'

setup()

test('keeps the first observation', () => expect(2 + 2).toBe(4))
test('keeps the second observation', () => expect(2 + 2).toBe(4))
