// Both callbacks belong to the runner module, even though each registration has its own call.
import { run } from './runner'

run('first')
run('second')
