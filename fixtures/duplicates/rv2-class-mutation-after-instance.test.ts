// Mutation of a class prototype after constructing an instance changes later method reads.
// Only the dependent pair is unresolved; the independent pair still compares.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable The two dependent bodies are identical and excluded for unknown class-object mutation.
// @vitest-dedup:participant L12 "mutated first"
// @vitest-dedup:participant L13 "mutated second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L14 "stable first"
// @vitest-dedup:participant L15 "stable second"
import { test, expect } from 'vitest';
test('mutated first', () => { void 'mutated-class'; class C { read() { return 1; } } const c = new C(); C.prototype.read = () => 2; expect(c.read()).toBe(2); });
test('mutated second', () => { void 'mutated-class'; class C { read() { return 1; } } const c = new C(); C.prototype.read = () => 2; expect(c.read()).toBe(2); });
test('stable first', () => { void 'stable-class'; class C { read() { return 1; } } const c = new C(); expect(c.read()).toBe(1); });
test('stable second', () => { void 'stable-class'; class C { read() { return 1; } } const c = new C(); expect(c.read()).toBe(1); });
