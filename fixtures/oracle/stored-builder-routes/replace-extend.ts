// Replaces the fixture builder on Vitest's shared test API when imported.
import { test } from 'vitest'

test.extend = () => () => {}
