import { beforeEach, expect, test } from 'vitest';
let ready = 0;
beforeEach(() => { ready = 1; });
beforeEach(() => { ready = 1; });
test('synchronous', () => { expect(ready).toBe(1); });
