const config = {
  test: { globals: process.env.VITEST_DEDUP_FORWARDED_GLOBALS !== 'off' },
}

export default config
