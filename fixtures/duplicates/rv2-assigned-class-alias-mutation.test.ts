// A class alias assigned later can mutate an instance's method after construction.
// Only the dependent pair is unresolved; the independent pair still compares.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable The two dependent bodies are identical and excluded for unknown class-object mutation.
// @vitest-dedup:participant L12 "alias first"
// @vitest-dedup:participant L13 "alias second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L14 "stable first"
// @vitest-dedup:participant L15 "stable second"
import { test, expect } from 'vitest';
test('alias first', () => { void 'alias-mutation'; class C { read() { return 1; } } const c = new C(); let K; K = C; K.prototype.read = () => 2; expect(c.read()).toBe(2); });
test('alias second', () => { void 'alias-mutation'; class C { read() { return 1; } } const c = new C(); let K; K = C; K.prototype.read = () => 2; expect(c.read()).toBe(2); });
test('stable first', () => { void 'stable-alias'; class C { read() { return 1; } } const c = new C(); expect(c.read()).toBe(1); });
test('stable second', () => { void 'stable-alias'; class C { read() { return 1; } } const c = new C(); expect(c.read()).toBe(1); });
