// A definedness assertion does not inspect an owned instance's class name.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L7 "first defined instance"
// @vitest-dedup:participant L8 "second defined instance"
import { test, expect } from 'vitest';
test('first defined instance', () => { class First { value = 1; } const c = new First(); expect(c).toBeDefined(); });
test('second defined instance', () => { class Second { value = 1; } const c = new Second(); expect(c).toBeDefined(); });
