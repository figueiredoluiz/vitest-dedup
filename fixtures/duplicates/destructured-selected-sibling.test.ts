// Selecting one key from a const object literal ignores the unselected siblings.
// Expected verdict: duplicate-test within each value group; the selected value stays
// load-bearing, so the two groups remain consolidation candidates of each other (changing
// chosen and the matching toBe assertion moves a test between them).
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L16 "selected one"
// @vitest-dedup:participant L17 "selected one with sibling"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L18 "selected two"
// @vitest-dedup:participant L19 "selected two with sibling"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L16 "selected one"
// @vitest-dedup:participant L18 "selected two"
import { test, expect } from 'vitest';
test('selected one', () => { const { chosen } = { chosen: 1 }; class C { read() { return chosen; } } expect(new C().read()).toBe(1); });
test('selected one with sibling', () => { const { chosen } = { chosen: 1, other: 5 }; class C { read() { return chosen; } } expect(new C().read()).toBe(1); });
test('selected two', () => { const { chosen } = { chosen: 2 }; class C { read() { return chosen; } } expect(new C().read()).toBe(2); });
test('selected two with sibling', () => { const { chosen } = { chosen: 2, other: 5 }; class C { read() { return chosen; } } expect(new C().read()).toBe(2); });
