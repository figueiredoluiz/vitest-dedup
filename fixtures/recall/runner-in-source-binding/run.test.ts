// RECALL negative -- no duplicate-test, shape: an imported runner destructures the in-source API.
//
// `runner.ts` is not an in-source test module, so `import.meta.vitest` is undefined there and
// Vitest fails the file; reading the binding as the framework's would pair `first` and `second`.
import { run } from './runner'

run()
