import { afterAll, afterEach, expect, test } from 'vitest'
let value = 0
let observedBeforeWrite = 0
afterEach(() => { value = 1 })
afterEach(() => { value = 1 })
test('eligible anchor', () => { expect(2 + 3).toBe(5) })
test('queues a later write', () => { value = 0; let work = Promise.resolve(); for (let i = 0; i < 12; i++) work = work.then(() => {}); work.then(() => { observedBeforeWrite = value; value = 2 }) })
afterAll(() => { expect([observedBeforeWrite, value]).toEqual([1, 1]) })
