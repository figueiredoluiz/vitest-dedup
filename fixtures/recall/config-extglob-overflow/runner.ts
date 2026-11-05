import { test, expect } from 'vitest';

/** Registers the duplicate pair through an imported runner. */
export function register() {
  test('uncertain first', () => { expect(4).toBe(4); });
  test('uncertain second', () => { expect(4).toBe(4); });
}
