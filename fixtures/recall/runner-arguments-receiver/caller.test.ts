// RECALL negative -- duplicate-test, shape: a runner whose registered callback reads the
// invocation's own `arguments`. What the callback sees depends on how it is invoked, so the
// invocation stays refused and the runner's scope is withheld. Control for
// runner-import-meta.
import { run } from './runner'

run('page')
