// Private calls keep their lexical brand; empty class frames retain outer owners.
// @vitest-dedup:extracted 4
// @vitest-dedup:not-runnable The wrong private method owner intentionally throws.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L12 "outer private call"
// @vitest-dedup:participant L13 "inner private call mistaken"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L14 "empty middle first"
// @vitest-dedup:participant L15 "empty middle renamed"
import { test, expect } from 'vitest';
test('outer private call', () => { void 'private-call-owner'; class O { #x() { return 1; } check(o) { class I { #y() { return 2; } get(v) { return v.#x(); } } return new I().get(o); } } expect(new O().check(new O())).toBe(1); });
test('inner private call mistaken', () => { void 'private-call-owner'; class O { #x() { return 1; } check(o) { class I { #y() { return 2; } get(v) { return v.#y(); } } return new I().get(o); } } expect(new O().check(new O())).toBe(1); });
test('empty middle first', () => { void ['private-empty-frame']; class O { #x = 1; check(o) { class M { make() { class I { #z = 3; get(v) { return v.#x; } } return new I(); } } return new M().make().get(o); } } expect(new O().check(new O())).toBe(1); });
test('empty middle renamed', () => { void ['private-empty-frame']; class Outer { #state = 1; check(o) { class Middle { make() { class Inner { #local = 3; get(v) { return v.#state; } } return new Inner(); } } return new Middle().make().get(o); } } expect(new Outer().check(new Outer())).toBe(1); });
