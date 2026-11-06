// Repeated invocations execute four tests but retain two source declarations: the written pair
// pairs with each other, and each invocation pairs with its own twin across calls.
import { run } from './runner'

run()
run()
