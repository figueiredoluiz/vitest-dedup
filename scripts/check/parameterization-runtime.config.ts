// Runs the owned parameterization cases instead of only collecting their registrations.
export default {
  test: {
    include: [
      'fixtures/duplicates/parameterization-contract.test.ts',
      'fixtures/recall/parameterization-row-order-*/main.test.ts',
      'fixtures/parameterization-runtime/*/main.test.ts',
      'fixtures/duplicates/normalization-stack.test.ts',
      'fixtures/duplicates/parameterization-inline-snapshots.test.ts',
      'fixtures/duplicates/parameterization-computed-keys.test.ts',
      'fixtures/duplicates/parameterization-data-selectors.test.ts',
      'fixtures/duplicates/parameterization-selector-operations.test.ts',
      'fixtures/duplicates/parameterization-large-literal.test.ts',
      'fixtures/duplicates/parameterization-row-data.test.ts',
      'fixtures/duplicates/parameterization-row-identity.test.ts',
      'fixtures/duplicates/parameterization-custom-snapshots.test.ts',
      'fixtures/duplicates/parameterization-modules.test.ts',
    ],
    maxWorkers: 1,
    fileParallelism: false,
  },
}
