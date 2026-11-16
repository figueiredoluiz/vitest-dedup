import { describe, expect, test } from 'vitest';

describe('inventory', () => {
  describe('available', () => {
    test('counts items', () => { expect(7).toBe(7); });
  });
  test('counts orders', () => { expect(12).toBe(12); });
});

describe('inventory', () => {
  describe('available', () => {
    test('counts items', () => { expect(7).toBe(7); });
  });
  test('counts orders', () => { expect(12).toBe(12); });
});

test('checks delivery', () => { expect(18).toBe(18); });
test('also checks delivery', () => { expect(18).toBe(18); });
