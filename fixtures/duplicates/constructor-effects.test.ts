// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The second constructor produces a different observed value.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L13 "store"
// @vitest-dedup:participant L14 "cart"
import { test, expect } from 'vitest';
const trace = []; class Store { constructor(value) { this.value = value; } load() { trace.push(this.value); } }
/** Supply the passing constructor value. */
function loadA() { return 1; }
/** Supply the distinct constructor value. */
function loadB() { return 2; }
test('store', () => { trace.length=0; const s = new Store(loadA()); s.load(); expect(trace).toEqual([1]); });
test('cart', () => { trace.length=0; const s = new Store(loadB()); s.load(); expect(trace).toEqual([1]); });
