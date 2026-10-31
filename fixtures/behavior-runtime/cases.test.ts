// Runtime claims the behavior trace's await/.then splice rests on, one test per claim, each
// outcome pinned in expected.json and asserted by `scripts/check/check-behavior-runtime.mjs`
// under the pinned Vitest it prints. Versions are recorded there at every run, not here.
// [status: current]
import { expect, test } from "vitest";

// A returned chain's continuation is the test's own behavior: its failing assertion fails the
// test, exactly as the awaited spelling's does. This is the pair the splice canonicalizes.
test("returned chain runs as part of the test", () => {
  return Promise.resolve().then(() => {
    expect(1).toBe(2);
  });
});

test("awaited twin", async () => {
  await Promise.resolve();
  expect(1).toBe(2);
});

// An unreturned chain is not awaited: the test body finishes before the microtask runs, so the
// continuation's write is invisible to it. Splicing this shape would claim an ordering the
// runtime does not have.
test("unreturned chain is not awaited", () => {
  let ran = false;
  Promise.resolve().then(() => {
    ran = true;
  });
  expect(ran).toBe(false);
});

// A second argument is a rejection handler, which observes a code path no `await` reaches: an
// `await` that rejects throws, it does not call a second function.
test("rejection handler observes the rejected path", () => {
  return Promise.reject(new Error("rejected")).then(
    () => {
      expect.unreachable("the fulfilled handler ran on a rejection");
    },
    () => {},
  );
});
