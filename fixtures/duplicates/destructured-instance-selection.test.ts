// A property selected from a local instance is not the whole instance.
// The conflicting assertions intentionally fail at runtime: inner is an Array, not C.
// @vitest-dedup:extracted 6
// @vitest-dedup:not-runnable The two selected-value conflicts each have one failing test.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L17 "selected array"
// @vitest-dedup:participant L18 "whole instance mistaken"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L19 "nested selected array"
// @vitest-dedup:participant L20 "nested whole instance mistaken"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L21 "direct instance first"
// @vitest-dedup:participant L22 "direct instance renamed"
import { test, expect } from 'vitest';
test('selected array', () => { void 'selected-instance-property'; class C { inner = []; } const { inner } = new C(); expect(inner.constructor.name).toBe('Array'); });
test('whole instance mistaken', () => { void 'selected-instance-property'; class C { inner = []; } const { inner } = new C(); expect('C').toBe('Array'); });
test('nested selected array', () => { void 'nested-selected-instance-property'; class C { nested = { inner: [] }; } const { nested: { inner } } = new C(); expect(inner.constructor.name).toBe('Array'); });
test('nested whole instance mistaken', () => { void 'nested-selected-instance-property'; class C { nested = { inner: [] }; } const { nested: { inner } } = new C(); expect('C').toBe('Array'); });
test('direct instance first', () => { void 'direct-instance-retention'; class C { read() { return 1; } } const value = new C(); expect(value.read()).toBe(1); });
test('direct instance renamed', () => { void 'direct-instance-retention'; class Renamed { read() { return 1; } } const instance = new Renamed(); expect(instance.read()).toBe(1); });
