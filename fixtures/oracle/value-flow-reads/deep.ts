import { test } from 'vitest'
/** The test API. */ function f0() { return test }
/** What f0 returns, one call deeper. */ function f1() { return f0() }
/** What f1 returns, one call deeper. */ function f2() { return f1() }
/** What f2 returns, one call deeper. */ function f3() { return f2() }
/** What f3 returns, one call deeper. */ function f4() { return f3() }
/** What f4 returns, one call deeper. */ function f5() { return f4() }
/** What f5 returns, one call deeper. */ function f6() { return f5() }
/** What f6 returns, one call deeper. */ function f7() { return f6() }
/** What f7 returns, one call deeper. */ function f8() { return f7() }
/** What f8 returns, one call deeper. */ function f9() { return f8() }
/** What f9 returns, one call deeper. */ function f10() { return f9() }
/** What f10 returns, one call deeper. */ function f11() { return f10() }
/** What f11 returns, one call deeper. */ function f12() { return f11() }
/** What f12 returns, one call deeper. */ function f13() { return f12() }
/** What f13 returns, one call deeper. */ function f14() { return f13() }
/** What f14 returns, one call deeper. */ function f15() { return f14() }
/** What f15 returns, one call deeper. */ function f16() { return f15() }
/** What f16 returns, one call deeper. */ function f17() { return f16() }
/** What f17 returns, one call deeper. */ function f18() { return f17() }
/** What f18 returns, one call deeper. */ function f19() { return f18() }
/** What f19 returns, one call deeper. */ function f20() { return f19() }
/** What f20 returns, one call deeper. */ function f21() { return f20() }
/** What f21 returns, one call deeper. */ function f22() { return f21() }
/** What f22 returns, one call deeper. */ function f23() { return f22() }
/** What f23 returns, one call deeper. */ function f24() { return f23() }
/** What f24 returns, one call deeper. */ function f25() { return f24() }
/** What f25 returns, one call deeper. */ function f26() { return f25() }
/** What f26 returns, one call deeper. */ function f27() { return f26() }
/** What f27 returns, one call deeper. */ function f28() { return f27() }
/** What f28 returns, one call deeper. */ function f29() { return f28() }
/** What f29 returns, one call deeper. */ function f30() { return f29() }
/** What f30 returns, one call deeper. */ function f31() { return f30() }
/** What f31 returns, one call deeper. */ function f32() { return f31() }
/** What f32 returns, one call deeper. */ function f33() { return f32() }
/** What f33 returns, one call deeper. */ function f34() { return f33() }
/** What f34 returns, one call deeper. */ function f35() { return f34() }
/** What f35 returns, one call deeper. */ function f36() { return f35() }
/** What f36 returns, one call deeper. */ function f37() { return f36() }
/** What f37 returns, one call deeper. */ function f38() { return f37() }
/** What f38 returns, one call deeper. */ function f39() { return f38() }
/** What f39 returns, one call deeper. */ function f40() { return f39() }
/** What f40 returns, one call deeper. */ function f41() { return f40() }
/** What f41 returns, one call deeper. */ function f42() { return f41() }
/** What f42 returns, one call deeper. */ function f43() { return f42() }
/** What f43 returns, one call deeper. */ function f44() { return f43() }
/** What f44 returns, one call deeper. */ function f45() { return f44() }
/** What f45 returns, one call deeper. */ function f46() { return f45() }
/** What f46 returns, one call deeper. */ function f47() { return f46() }
/** What f47 returns, one call deeper. */ function f48() { return f47() }
/** What f48 returns, one call deeper. */ function f49() { return f48() }
/** What f49 returns, one call deeper. */ function f50() { return f49() }
/** What f50 returns, one call deeper. */ function f51() { return f50() }
/** What f51 returns, one call deeper. */ function f52() { return f51() }
/** What f52 returns, one call deeper. */ function f53() { return f52() }
/** What f53 returns, one call deeper. */ function f54() { return f53() }
/** What f54 returns, one call deeper. */ function f55() { return f54() }
/** What f55 returns, one call deeper. */ function f56() { return f55() }
/** What f56 returns, one call deeper. */ function f57() { return f56() }
/** What f57 returns, one call deeper. */ function f58() { return f57() }
/** What f58 returns, one call deeper. */ function f59() { return f58() }
/** What f59 returns, one call deeper. */ function f60() { return f59() }
/** Past the deepest chain the analyzer follows. */
export function deep() { return f60() }
/** Within it. */
export function shallow() { return f2() }
