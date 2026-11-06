// RECALL negative -- no duplicate-test, shape: a runner's hoisted function reads the in-source
// names before their declaration runs. `inner()` runs first, so `test` is in its temporal dead zone
// and Vitest fails the file; reading the calls as the framework's would pair `first` and `second`.
import { run } from './runner'

run()
