// A body deeper than the behavior collector's recursion cap must not be compared on its
// truncated trace. Each of the first two tests nests 260 unbraced `if (true)` statements -- no
// brackets, so the parser's nesting limit never refuses it -- and inside the innermost block
// constructs a class whose constructor appends to `trace`: `push(1)` in the first, `unshift(2)`
// in the second. Both then assert `trace` equals `[1]`. The depth cap drops that region from
// the behavior trace on both sides, so the two traces compare equal while the bodies differ --
// under Node v26.0.0 / Vitest 5.0.5 this file runs 3 passed / 1 failed (`deep unshift` is the
// failure: `unshift(2)` leaves `[2]`), which is the difference the truncated trace cannot see.
// The fix marks such bodies unresolved, so this pair is not-compared rather than a false
// near-duplicate. The last two tests are the shallow retention pair: the same construction at
// two levels of nesting, where the constructor effect IS collected, and an inert `1;` statement
// (no behavior event, visible in alpha) makes the bodies differ -- `near-duplicate-test` must
// keep firing there, proving the unresolved verdict silences only the truncated bodies and not
// the rule.
//
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable the participants differ only inside the region the depth cap dropped from the behavior trace: their collected events are equal by construction of the defect, so no relation over the emitted views can separate them as a behavior variable -- alpha does distinguish the pair (the raw-text fallback keeps the deep difference), which is exactly how the unfixed analyzer read the equal traces as a false near-duplicate. The guard is check-duplicates-corpus.mjs's eligibility assertion: not-compared holds only while the solo run reports the unresolved contexts the truncation produces.
// @vitest-dedup:participant L25 "deep push"
// @vitest-dedup:participant L33 "deep unshift"
// @vitest-dedup:expect rule=near-duplicate-test verdict=similar-uncertain severity=warning
// @vitest-dedup:participant L41 "shallow sink"
// @vitest-dedup:participant L49 "shallow sink inert marker"
import { test, expect } from 'vitest';
test('deep push', () => {
  const trace: number[] = [];
  if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) {
    class Sink { constructor() { trace.push(1); } }
    new Sink();
    expect(trace).toEqual([1]);
  }
});
test('deep unshift', () => {
  const trace: number[] = [];
  if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) if (true) {
    class Sink { constructor() { trace.unshift(2); } }
    new Sink();
    expect(trace).toEqual([1]);
  }
});
test('shallow sink', () => {
  const trace: number[] = [];
  if (true) if (true) {
    class Sink { constructor() { trace.push(1); } }
    new Sink();
    expect(trace).toEqual([1]);
  }
});
test('shallow sink inert marker', () => {
  const trace: number[] = [];
  1;
  if (true) if (true) {
    class Sink { constructor() { trace.push(1); } }
    new Sink();
    expect(trace).toEqual([1]);
  }
});
