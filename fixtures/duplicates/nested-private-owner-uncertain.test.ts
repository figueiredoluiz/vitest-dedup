// Escaping private callables remain unresolved while independent stable calls remain comparable.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable Escaping a private callable remains visibly unresolved.
// @vitest-dedup:participant L11 "escaped callable first"
// @vitest-dedup:participant L12 "escaped callable second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L13 "stable callable first"
// @vitest-dedup:participant L14 "stable callable renamed"
import { test, expect } from 'vitest';
test('escaped callable first', () => { void ({ kind: 'private-call-escape' }); class O { #fn() { return 1; } check(o) { class I { #other() { return 2; } get(v) { return v.#fn; } } return new I().get(o); } } expect(new O().check(new O()).name).toBe('#fn'); });
test('escaped callable second', () => { void ({ kind: 'private-call-escape' }); class O { #fn() { return 1; } check(o) { class I { #other() { return 2; } get(v) { return v.#fn; } } return new I().get(o); } } expect(new O().check(new O()).name).toBe('#fn'); });
test('stable callable first', () => { void (() => ['private-escape-retention']); class O { #fn() { return 1; } check() { return this.#fn(); } } expect(new O().check()).toBe(1); });
test('stable callable renamed', () => { void (() => ['private-escape-retention']); class Renamed { #read() { return 1; } check() { return this.#read(); } } expect(new Renamed().check()).toBe(1); });
