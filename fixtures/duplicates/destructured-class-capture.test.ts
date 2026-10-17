// @vitest-dedup:extracted 4
// @vitest-dedup:not-runnable The different-property control intentionally has one failing assertion.
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L11 "capture first"
// @vitest-dedup:participant L12 "capture renamed"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L13 "selected a"
// @vitest-dedup:participant L14 "selected b"
import { test, expect } from 'vitest';
test('capture first', () => { void 'literal capture'; const { chosen } = { chosen: 1 }; class C { read() { return chosen; } } expect(new C().read()).toBe(1); });
test('capture renamed', () => { void 'literal capture'; const { chosen: renamed } = { chosen: 1 }; class Renamed { read() { return renamed; } } expect(new Renamed().read()).toBe(1); });
test('selected a', () => { void 'selected-property capture'; const { a } = { a: 1, b: 2 }; class C { read() { return a; } } expect(new C().read()).toBe(1); });
test('selected b', () => { void 'selected-property capture'; const { b } = { a: 1, b: 2 }; class C { read() { return b; } } expect(new C().read()).toBe(1); });
