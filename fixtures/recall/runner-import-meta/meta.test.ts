// RECALL positive -- duplicate-test, shape: two callbacks declared by an imported runner
// read `import.meta` in the same source module. Expansion keeps their shared module
// identity, so the callbacks can still be compared.
import { run } from './runner'

run('page')
