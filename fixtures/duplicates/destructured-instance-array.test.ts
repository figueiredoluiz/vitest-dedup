// The first item of an iterable C is selected as a value, not treated as C itself.
// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The mistaken assertion intentionally fails.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L9 "array selected"
// @vitest-dedup:participant L10 "array whole mistaken"
import { test, expect } from 'vitest';
test('array selected', () => { class C extends Array { constructor() { super(); this.push([]); } } const [inner] = new C(); expect(inner.constructor.name).toBe('Array'); });
test('array whole mistaken', () => { class C extends Array { constructor() { super(); this.push([]); } } const [inner] = new C(); expect('C').toBe('Array'); });
