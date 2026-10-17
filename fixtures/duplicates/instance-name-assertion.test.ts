// A whole owned instance passed to an assertion can expose its class name.
// The second assertion fails and cannot be reported as a redundant test.
// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The second instance has a different constructor name.
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L9 "first instance name"
// @vitest-dedup:participant L10 "second instance name"
import { test, expect } from 'vitest';
test('first instance name', () => { class First { value = 1; } const c = new First(); expect(Object.getPrototypeOf(c).constructor.name).toBe('First'); });
test('second instance name', () => { class Second { value = 1; } const c = new Second(); expect(Object.getPrototypeOf(c).constructor.name).toBe('First'); });
