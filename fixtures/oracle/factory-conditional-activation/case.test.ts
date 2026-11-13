import {make} from './factory'
const disabled=make(false)
disabled.run()
disabled.skip()
const enabled=make(true)
enabled.run()
enabled.skip()
