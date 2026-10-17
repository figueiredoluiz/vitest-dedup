// Three lexical class frames must retain the private owner's relative depth.
// @vitest-dedup:extracted 4
// @vitest-dedup:not-runnable The middle-owner mistake intentionally throws.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L12 "outer owner at depth two"
// @vitest-dedup:participant L13 "middle owner mistaken"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L14 "deep owner first"
// @vitest-dedup:participant L15 "deep owner renamed"
import { test, expect } from 'vitest';
test('outer owner at depth two', () => { void 'private-depth-negative'; class O { #x = 1; check(o) { class M { #y = 2; make() { class I { #z = 3; get(v) { return v.#x; } } return new I(); } } return new M().make().get(o); } } expect(new O().check(new O())).toBe(1); });
test('middle owner mistaken', () => { void 'private-depth-negative'; class O { #x = 1; check(o) { class M { #y = 2; make() { class I { #z = 3; get(v) { return v.#y; } } return new I(); } } return new M().make().get(o); } } expect(new O().check(new O())).toBe(1); });
test('deep owner first', () => { void ['private-depth-positive']; class O { #x = 1; check(o) { class M { #y = 2; make() { class I { #z = 3; get(v) { return v.#x; } } return new I(); } } return new M().make().get(o); } } expect(new O().check(new O())).toBe(1); });
test('deep owner renamed', () => { void ['private-depth-positive']; class Renamed { #state = 1; check(o) { class Middle { #local = 2; make() { class Inner { #own = 3; get(v) { return v.#state; } } return new Inner(); } } return new Middle().make().get(o); } } expect(new Renamed().check(new Renamed())).toBe(1); });
