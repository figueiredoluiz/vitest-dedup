import { expect, test } from 'vitest'
import './eager'

test('retained left', () => expect(2).toBe(2))
test('retained right', () => expect(2).toBe(2))
