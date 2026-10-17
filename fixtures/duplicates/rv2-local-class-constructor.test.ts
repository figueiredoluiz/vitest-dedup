// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The second constructor produces a different observed trace.
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L9 "one"
// @vitest-dedup:participant L10 "two"
import { test, expect, beforeEach } from 'vitest';
const trace = [];
beforeEach(() => { trace.length = 0; });
test('one', () => { class C { constructor() { trace.push(1); } } new C(); expect(trace).toEqual([1]); });
test('two', () => { class C { constructor() { trace.push(2); } } new C(); expect(trace).toEqual([1]); });
