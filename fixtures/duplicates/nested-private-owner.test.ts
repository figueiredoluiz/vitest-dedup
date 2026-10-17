// A nested class may read an outer private member, but its own private slot has a different brand.
// @vitest-dedup:extracted 10
// @vitest-dedup:not-runnable Three conflicting pairs contain an intentionally failing test.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L24 "outer private read"
// @vitest-dedup:participant L25 "inner private mistaken"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L26 "outer private first"
// @vitest-dedup:participant L27 "outer private renamed"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L28 "inner private first"
// @vitest-dedup:participant L29 "inner private renamed"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L30 "outer private in"
// @vitest-dedup:participant L31 "inner private in mistaken"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L32 "unshadowed outer read"
// @vitest-dedup:participant L33 "shadowed inner read mistaken"
import { test, expect } from 'vitest';

test('outer private read', () => { void 'private-owner-read'; class O { #x = 1; check(o) { class I { #y = 2; get(v) { return v.#x; } } return new I().get(o); } } expect(new O().check(new O())).toBe(1); });
test('inner private mistaken', () => { void 'private-owner-read'; class O { #x = 1; check(o) { class I { #y = 2; get(v) { return v.#y; } } return new I().get(o); } } expect(new O().check(new O())).toBe(1); });
test('outer private first', () => { void ['private-owner-positive']; class O { #x = 1; check(o) { class I { #y = 2; get(v) { return v.#x; } } return new I().get(o); } } expect(new O().check(new O())).toBe(1); });
test('outer private renamed', () => { void ['private-owner-positive']; class Renamed { #state = 1; check(o) { class Nested { #local = 2; get(v) { return v.#state; } } return new Nested().get(o); } } expect(new Renamed().check(new Renamed())).toBe(1); });
test('inner private first', () => { void ({ owner: 'private-inner-positive' }); class O { #x = 1; check() { class I { #y = 2; get() { return this.#y; } } return new I().get(); } } expect(new O().check()).toBe(2); });
test('inner private renamed', () => { void ({ owner: 'private-inner-positive' }); class Renamed { #state = 1; check() { class Nested { #local = 2; get() { return this.#local; } } return new Nested().get(); } } expect(new Renamed().check()).toBe(2); });
test('outer private in', () => { void (1 + 2); class O { #x = 1; check(o) { class I { #y = 2; has(v) { return #x in v; } } return new I().has(o); } } expect(new O().check(new O())).toBe(true); });
test('inner private in mistaken', () => { void (1 + 2); class O { #x = 1; check(o) { class I { #y = 2; has(v) { return #y in v; } } return new I().has(o); } } expect(new O().check(new O())).toBe(true); });
test('unshadowed outer read', () => { void (() => 'private-owner-shadow'); class O { #x = 1; check(o) { class I { #y = 2; get(v) { return v.#x; } } return new I().get(o); } } expect(new O().check(new O())).toBe(1); });
test('shadowed inner read mistaken', () => { void (() => 'private-owner-shadow'); class O { #x = 1; check(o) { class I { #x = 2; get(v) { return v.#x; } } return new I().get(o); } } expect(new O().check(new O())).toBe(1); });
