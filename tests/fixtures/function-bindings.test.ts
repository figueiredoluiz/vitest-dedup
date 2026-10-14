import { test, expect } from 'vitest'

test('a', () => { function a(){return 1} function b(){return 2} expect(a()).toBe(1) })
test('b', () => { function a(){return 1} function b(){return 2} expect(b()).toBe(1) })
