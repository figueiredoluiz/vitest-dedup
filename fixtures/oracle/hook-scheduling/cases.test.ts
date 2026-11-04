import { afterEach, test, expect } from 'vitest';
let value = 0; let observed = 0;
afterEach(() => { value = 1; });
afterEach(() => { value = 1; });
afterEach(() => { expect(observed).toBe(1); });
test('queues observation', () => { let work = Promise.resolve(); for (let i = 0; i < 18; i++) work = work.then(() => {}); work.then(() => { observed = 1; }); expect(value).toBe(0); });
