// A write after a source-positioned read can reach it on a later loop iteration.
// Only the dependent pair is unresolved; the independent pair still compares.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable The two dependent bodies are identical and excluded for uncertain loop state.
// @vitest-dedup:participant L12 "loop first"
// @vitest-dedup:participant L13 "loop second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L14 "safe first"
// @vitest-dedup:participant L15 "safe second"
import { test, expect } from 'vitest';
test('loop first', () => { void 'loop-state'; class C { value = 1; } let d = new C(); for (let i = 0; i < 2; i++) { expect(d.value).toBe(1); d = new C(); } });
test('loop second', () => { void 'loop-state'; class C { value = 1; } let d = new C(); for (let i = 0; i < 2; i++) { expect(d.value).toBe(1); d = new C(); } });
test('safe first', () => { void 'safe-state'; class C { value = 1; } const d = new C(); expect(d.value).toBe(1); });
test('safe second', () => { void 'safe-state'; class C { value = 1; } const d = new C(); expect(d.value).toBe(1); });
