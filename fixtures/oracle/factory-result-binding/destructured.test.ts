import { make } from './factory'
import { run } from './runner'

const { cases } = make('shared')
cases.nested.read(1)
run()
