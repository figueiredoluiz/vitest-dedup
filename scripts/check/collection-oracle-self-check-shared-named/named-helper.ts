// Reads a missing module's export once, when it loads, and keeps the value.
import { enabled } from './missing-module'

export const flag = enabled
