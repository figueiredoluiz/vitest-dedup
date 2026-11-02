// Identical byte offsets in two helper modules identify two declarations, never one.
import { run as left } from './left'
import { run as right } from './right'

left()
right()
