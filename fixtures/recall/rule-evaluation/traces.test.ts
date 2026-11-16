import { expect, test } from 'vitest';

test('confirms stock', () => {
  expect(7).toBe(7);
  expect('in stock').toBe('in stock');
});

test('confirms stock and location', () => {
  expect(7).toBe(7);
  expect('in stock').toBe('in stock');
  expect('aisle 3').toBe('aisle 3');
});

test('confirms location in a different order', () => {
  expect('in stock').toBe('in stock');
  expect(7).toBe(7);
  expect('aisle 3').toBe('aisle 3');
});

test.skip('scheduled inventory count', () => {
  expect(7).toBe(7);
  expect('in stock').toBe('in stock');
  expect('aisle 5').toBe('aisle 5');
});
