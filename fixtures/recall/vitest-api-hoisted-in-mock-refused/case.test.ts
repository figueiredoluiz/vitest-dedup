// RECALL negative -- `vi.hoisted` inside a `vi.mock` factory makes Vitest refuse the file
// ("Cannot call vi.hoisted() inside vi.mock"), so its tests are not compared. Returning the
// object literal directly from the factory makes them a duplicate.
import { test, expect, vi } from 'vitest'
import { value } from './dep'

vi.mock('./dep', () => vi.hoisted(() => ({ value: 'mocked' })))
test('reads the mock', () => expect(value).toBe('mocked'))
test('reads the mock, again', () => expect(value).toBe('mocked'))
