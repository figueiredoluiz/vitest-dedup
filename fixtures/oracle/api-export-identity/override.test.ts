import { test, beforeEach } from './override';
import { expect } from 'vitest';
let ready = 0;
beforeEach(() => { ready = 1; });
test('first', () => { expect(ready).toBe(1); });
test('second', () => { expect(ready).toBe(1); });
