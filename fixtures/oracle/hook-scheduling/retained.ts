import { beforeEach, expect, test } from 'vitest';
let ready = 0;
/** Registers synchronous setup and its observable test. */
export function install() {
  beforeEach(() => { ready = 1; });
  beforeEach(() => { ready = 1; });
  test('retained', () => { expect(ready).toBe(1); });
}
/** Remains uninvoked so its async callback cannot affect scheduling evidence. */
export function unused() {
  test('never registered', async () => { expect(ready).toBe(0); });
}
