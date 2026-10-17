// An escaped instance exposes its nominal constructor name outside the owned callback.
// Both tests pass. The name can be parameterized, but the tests are not redundant.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L9 "first escaped name"
// @vitest-dedup:participant L10 "second escaped name"
import { test, expect } from 'vitest';

test('first escaped name', () => { class First { constructor() { globalThis.ref = this; } } new First(); expect(globalThis.ref.constructor.name).toBe('First'); });
test('second escaped name', () => { class Second { constructor() { globalThis.ref = this; } } new Second(); expect(globalThis.ref.constructor.name).toBe('Second'); });
