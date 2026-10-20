// RECALL variant -- duplicate-test; an inline project whose `dir` leaves the analysis root.
//
// `../outside` is a sibling directory, not part of the tree being analyzed. The project cannot be
// scoped to it, so it is reported as unresolved: the duplicate pair in arith.test.ts is still
// found, and the run is incomplete. The sibling `duplicate-test-project-dir-dotdot-inside-root`
// spells a `dir` that folds back inside the root and runs complete.
export default {
  test: {
    projects: [{ test: { name: 'outside', dir: '../outside', include: ['**/*.test.ts'] } }],
  },
}
