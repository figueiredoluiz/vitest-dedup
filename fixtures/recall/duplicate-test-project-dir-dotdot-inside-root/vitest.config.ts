// RECALL variant -- duplicate-test; retention control for project directory confinement.
//
// `dir` is spelled with a `..` segment that folds back inside the analysis root, so it is the
// ordinary `./pkg` project: scoped, complete, and the duplicate pair is found. The sibling
// `duplicate-test-project-dir-outside-root` spells a `dir` that really leaves the root.
export default {
  test: {
    projects: [{ test: { name: 'pkg', dir: './pkg/../pkg', include: ['src/**/*.test.ts'] } }],
  },
}
