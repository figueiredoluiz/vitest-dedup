// Renaming a private owner must preserve brand checks and nearest-scope reads.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L10 "brand first"
// @vitest-dedup:participant L11 "brand renamed"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L12 "shadow first"
// @vitest-dedup:participant L13 "shadow renamed"
import { test, expect } from 'vitest';
test('brand first', () => { void 'private-brand-positive'; class O { #x = 1; check(o) { class I { #y = 2; has(v) { return #x in v; } } return new I().has(o); } } expect(new O().check(new O())).toBe(true); });
test('brand renamed', () => { void 'private-brand-positive'; class Renamed { #state = 1; check(o) { class Nested { #local = 2; has(v) { return #state in v; } } return new Nested().has(o); } } expect(new Renamed().check(new Renamed())).toBe(true); });
test('shadow first', () => { void ['private-shadow-positive']; class O { #x = 1; check() { class I { #x = 2; get() { return this.#x; } } return new I().get(); } } expect(new O().check()).toBe(2); });
test('shadow renamed', () => { void ['private-shadow-positive']; class Renamed { #state = 1; check() { class Nested { #local = 2; get() { return this.#local; } } return new Nested().get(); } } expect(new Renamed().check()).toBe(2); });
