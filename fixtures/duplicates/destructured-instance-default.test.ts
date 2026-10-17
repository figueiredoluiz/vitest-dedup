// The defaulted property value is an Array, not the whole C instance.
// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The mistaken assertion intentionally fails.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L9 "default selected"
// @vitest-dedup:participant L10 "default whole mistaken"
import { test, expect } from 'vitest';
test('default selected', () => { class C { inner = undefined; } const { inner = [] } = new C(); expect(inner.constructor.name).toBe('Array'); });
test('default whole mistaken', () => { class C { inner = undefined; } const { inner = [] } = new C(); expect('C').toBe('Array'); });
