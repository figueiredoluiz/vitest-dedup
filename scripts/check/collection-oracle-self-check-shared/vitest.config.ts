// Without isolation both files share one load of the stub; both must count as reading it.
export default { test: { isolate: false, maxWorkers: 1, fileParallelism: false } }
