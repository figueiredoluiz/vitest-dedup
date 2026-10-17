// Escaping this exposes the nominal class name; the second assertion fails at runtime.
// The observed class name can vary as a parameter, but the pair is not redundant.
// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The second escaped instance has a different constructor name.
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L10 "first escaped mismatch"
// @vitest-dedup:participant L11 "second escaped mismatch"
import { test, expect } from 'vitest';

test('first escaped mismatch', () => { void 'name-mismatch'; class First { constructor() { globalThis.ref = this; } } new First(); expect(globalThis.ref.constructor.name).toBe('First'); });
test('second escaped mismatch', () => { void 'name-mismatch'; class Second { constructor() { globalThis.ref = this; } } new Second(); expect(globalThis.ref.constructor.name).toBe('First'); });
