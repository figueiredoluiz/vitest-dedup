// RECALL positive -- duplicate-test: a flat 4,000-export table retains its final binding.
// Changing one assertion below breaks the pair while leaving both tests comparable.
import { expect, test } from 'vitest'
import { setupApp } from './setup'

setupApp()

test('clock: first check', () => { expect([3, 1, 2].sort()).toEqual([1, 2, 3]) })
test('clock: second check', () => { expect([3, 1, 2].sort()).toEqual([1, 2, 3]) })
