// Execute the owned hook outcomes; collection alone does not run these assertions.
export default {
  test: {
    include: [
      'fixtures/recall/redundant-hook-lexical-overwrite/main.test.ts',
      'fixtures/oracle/redundant-hooks/*.test.ts',
      'fixtures/oracle/hook-proof-boundaries/*.test.ts',
    ],
    setupFiles: ['./fixtures/oracle/hook-proof-boundaries/setup.ts'],
    maxWorkers: 1,
    fileParallelism: false,
  },
}
