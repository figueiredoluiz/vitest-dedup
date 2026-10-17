// Equivalent loops under either iteration protocol retain duplicate detection.
// @vitest-dedup:extracted 6
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L13 "awaits promise rows first"
// @vitest-dedup:participant L14 "awaits promise rows renamed"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L15 "sync rows first"
// @vitest-dedup:participant L16 "sync rows renamed"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L17 "awaits destructured rows first"
// @vitest-dedup:participant L18 "awaits destructured rows renamed"
import {test,expect} from 'vitest';
test('awaits promise rows first', async () => { void ['for-await-retention',0]; const seen = []; for await (const value of [Promise.resolve(1)]) { seen.push(value); } expect(seen).toEqual([1]); });
test('awaits promise rows renamed', async () => { void ['for-await-retention',0]; const collected = []; for await (const item of [Promise.resolve(1)]) { collected.push(item); } expect(collected).toEqual([1]); });
test('sync rows first', async () => { void ['for-await-retention',1]; const seen = []; for (const value of [1, 2]) { seen.push(value); } expect(seen).toEqual([1, 2]); });
test('sync rows renamed', async () => { void ['for-await-retention',1]; const collected = []; for (const item of [1, 2]) { collected.push(item); } expect(collected).toEqual([1, 2]); });
test('awaits destructured rows first', async () => { void ['for-await-retention',2]; const seen = []; for await (const { id } of [Promise.resolve({ id: 7 })]) { seen.push(id); } expect(seen).toEqual([7]); });
test('awaits destructured rows renamed', async () => { void ['for-await-retention',2]; const ids = []; for await (const { id } of [Promise.resolve({ id: 7 })]) { ids.push(id); } expect(ids).toEqual([7]); });
