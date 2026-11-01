// Subsumption requires compatible fixture and table input identities.
// @vitest-dedup:extracted 3
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable the default engine refuses the unresolved table populations; the owned Rust controls require refusal for both table APIs and retain a resolved-row comparison
// @vitest-dedup:participant L9 "a"
// @vitest-dedup:participant L10 "b"
import { test, expect } from 'vitest'
function loadA() { return [1] }; function loadB() { return [2] }; function f(n) { return 1 }; function g(n) { return 2 }
test.each(loadA())('a', n => { expect(f(n)).toBe(1) })
test.each(loadB())('b', n => { expect(f(n)).toBe(1); expect(g(n)).toBe(2) })
test('plain control', () => expect('table-population-control').toBe('table-population-control'))
