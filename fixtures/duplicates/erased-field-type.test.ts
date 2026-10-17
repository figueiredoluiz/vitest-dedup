// TypeScript field annotations do not change runtime class behavior.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L7 "typed field"
// @vitest-dedup:participant L8 "untyped field"
import { test, expect } from 'vitest';
test('typed field', () => { class C { value: number = 1; } expect(new C().value).toBe(1); });
test('untyped field', () => { class C { value = 1; } expect(new C().value).toBe(1); });
