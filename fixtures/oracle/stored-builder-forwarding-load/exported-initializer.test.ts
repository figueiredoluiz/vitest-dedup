import { expect, test } from 'vitest'
import './exported-initializer-helper'
test('exported initializer control', () => expect(7).toBe(7))
