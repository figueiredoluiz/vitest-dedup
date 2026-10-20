// RECALL variant -- duplicate-test; a `test.projects` entry that leaves the analysis root.
//
// `../outside` is a sibling directory, not part of the tree being analyzed. Its project config is
// never read, and the entry is reported as unresolved, so the run is incomplete even though the
// duplicate pair in arith.test.ts is still found. Its siblings with a project directory kept inside
// the root (`duplicate-test-projects-dir`, `duplicate-test-project-dir-dotdot-inside-root`) run
// complete.
export default {
  test: {
    projects: ['../outside'],
  },
}
