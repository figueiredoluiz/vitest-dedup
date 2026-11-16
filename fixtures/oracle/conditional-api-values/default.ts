import { test } from 'vitest'
export default process.env.ORACLE_OFF === 'true' ? test.skip : (..._args: unknown[]) => {}
