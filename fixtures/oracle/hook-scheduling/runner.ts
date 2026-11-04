import { beforeEach, expect, test } from 'vitest';
let ready = 0;
let other = 0;
/** Registers async tests under the first lexical setup state. */
export function installOne() {
  beforeEach(() => { ready = 1; });
  beforeEach(() => { ready = 1; });
  test('first', async () => { expect(true).toBe(true); });
  test('second', async () => { expect(true).toBe(true); });
}
/** Registers the same test bodies under a distinct lexical setup state. */
export function installTwo() {
  beforeEach(() => { other = 2; });
  beforeEach(() => { other = 2; });
  test('first', async () => { expect(true).toBe(true); });
  test('second', async () => { expect(true).toBe(true); });
}
