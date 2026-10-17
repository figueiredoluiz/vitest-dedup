// With no intervening write, renaming a selected property binding preserves the result.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L7 "one"
// @vitest-dedup:participant L8 "two"
import { test, expect } from 'vitest';
test('one', () => { void 'destructured-retain'; class C { inner = []; } const { inner } = new C(); expect(inner.length).toBe(0); });
test('two', () => { void 'destructured-retain'; class C { inner = []; } const { inner: selected } = new C(); expect(selected.length).toBe(0); });
