// Without isolation a helper caches a stub value once; every file using it must count as reading the stub.
export default { test: { isolate: false, maxWorkers: 1, fileParallelism: false } }
