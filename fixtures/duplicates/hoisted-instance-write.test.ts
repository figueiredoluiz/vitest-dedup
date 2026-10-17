// A hoisted function declared after the assertion can reassign the instance before it.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable The two bodies are identical and excluded for a deferred instance write.
// @vitest-dedup:participant L11 "hoisted first"
// @vitest-dedup:participant L12 "hoisted second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L13 "safe hoisted first"
// @vitest-dedup:participant L14 "safe hoisted second"
import { test, expect } from 'vitest';
test('hoisted first', () => { void 'hoisted-write'; class C { value = 1; } let d = new C(); reset(); expect(d.value).toBe(1); function reset() { d = new C(); } });
test('hoisted second', () => { void 'hoisted-write'; class C { value = 1; } let d = new C(); reset(); expect(d.value).toBe(1); function reset() { d = new C(); } });
test('safe hoisted first', () => { void 'safe-hoisted'; class C { value = 1; } const d = new C(); expect(d.value).toBe(1); });
test('safe hoisted second', () => { void 'safe-hoisted'; class C { value = 1; } const d = new C(); expect(d.value).toBe(1); });
