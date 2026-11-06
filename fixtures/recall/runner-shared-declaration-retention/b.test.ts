// Reusing that same declaration in a second caller runs a second, identical test: the two
// invocations are distinct runtime cases over one source declaration, and the identical bodies
// under equivalent contexts pair across the files.
import { run } from './runner'

run()
