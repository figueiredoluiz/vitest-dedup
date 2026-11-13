import { make } from './factory'
import { run } from './runner'

const result = make('shared')
result.cases.nested.read(1)
run()
