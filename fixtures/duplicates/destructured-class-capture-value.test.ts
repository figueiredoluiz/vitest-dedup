// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L6 "value one"
// @vitest-dedup:participant L7 "value two"
import { test, expect } from 'vitest';
test('value one', () => { void 'selected literal value'; const { chosen } = { chosen: 1 }; class C { read() { return chosen; } } expect(new C().read()).toBe(1); });
test('value two', () => { void 'selected literal value'; const { chosen } = { chosen: 2 }; class C { read() { return chosen; } } expect(new C().read()).toBe(2); });
