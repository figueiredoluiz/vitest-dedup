// The iteration protocol a for-of loop selects must survive comparison.
// @vitest-dedup:extracted 8
// @vitest-dedup:not-runnable Each pair intentionally applies the same assertion to both protocols: the sync loop yields a Promise, rejects an async-only source, or yields [1] where the async protocol yields [2].
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates for-await
// @vitest-dedup:participant L21 "await unwraps each value"
// @vitest-dedup:participant L22 "sync yields the promise itself"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates for-await
// @vitest-dedup:participant L23 "await iterates the async generator"
// @vitest-dedup:participant L24 "sync rejects the async generator"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates for-await
// @vitest-dedup:participant L25 "await reads the async iterator"
// @vitest-dedup:participant L26 "sync rejects the async iterator"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates for-await
// @vitest-dedup:participant L27 "await prefers the async protocol"
// @vitest-dedup:participant L28 "sync cannot produce the async protocol result"
import {test,expect} from 'vitest';
test('await unwraps each value', async () => { void 'await-unwraps'; const seen = []; for await (const value of [Promise.resolve(1)]) { seen.push(value); } expect(seen).toEqual([1]); });
test('sync yields the promise itself', async () => { void 'await-unwraps'; const seen = []; for (const value of [Promise.resolve(1)]) { seen.push(value); } expect(seen).toEqual([1]); });
test('await iterates the async generator', async () => { void 'await-generator'; const source = (async function* () { yield 1; yield 2; })(); const seen = []; for await (const value of source) { seen.push(value); } expect(seen).toEqual([1, 2]); });
test('sync rejects the async generator', async () => { void 'await-generator'; const source = (async function* () { yield 1; yield 2; })(); const seen = []; for (const value of source) { seen.push(value); } expect(seen).toEqual([1, 2]); });
test('await reads the async iterator', async () => { void 'await-iterator'; const source = { [Symbol.asyncIterator]() { let step = 0; return { next: () => Promise.resolve({ value: ++step, done: step > 2 }) }; } }; const seen = []; for await (const value of source) { seen.push(value); } expect(seen).toEqual([1, 2]); });
test('sync rejects the async iterator', async () => { void 'await-iterator'; const source = { [Symbol.asyncIterator]() { let step = 0; return { next: () => Promise.resolve({ value: ++step, done: step > 2 }) }; } }; const seen = []; for (const value of source) { seen.push(value); } expect(seen).toEqual([1, 2]); });
test('await prefers the async protocol', async () => { void 'await-dual-protocol'; const source = { *[Symbol.iterator]() { yield 1; }, async *[Symbol.asyncIterator]() { yield 2; } }; const seen = []; for await (const value of source) { seen.push(value); } expect(seen).toEqual([2]); });
test('sync cannot produce the async protocol result', async () => { void 'await-dual-protocol'; const source = { *[Symbol.iterator]() { yield 1; }, async *[Symbol.asyncIterator]() { yield 2; } }; const seen = []; for (const value of source) { seen.push(value); } expect(seen).toEqual([2]); });
