// Owned reporter corpus: one finding must retain both source locations under URI encoding.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L8 "keeps the first location"
// @vitest-dedup:participant L12 "keeps the second location"
import { test, expect } from 'vitest';

test('keeps the first location', () => {
  expect(23471 + 1).toBe(23472);
});

test('keeps the second location', () => {
  expect(23471 + 1).toBe(23472);
});
