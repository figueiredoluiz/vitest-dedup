// RECALL negative -- duplicate-test: required table exceeds the nesting budget.
// Removing the unused nested declaration from imports.d.ts restores the duplicate pair.
import { expect, test } from 'vitest'
import { setupApp } from './setup'

setupApp()

test('clock: first check', () => { expect([3, 1, 2].sort()).toEqual([1, 2, 3]) })
test('clock: second check', () => { expect([3, 1, 2].sort()).toEqual([1, 2, 3]) })
