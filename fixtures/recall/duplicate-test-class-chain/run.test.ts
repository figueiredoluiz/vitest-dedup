// RECALL positive -- duplicate-test expected, shape: classes that each read the one below twice.
//
// Rendering these by content doubles per level, so a run that does not bound it never finishes.
import { test } from 'vitest'

declare function use(value: unknown): void

test('one', () => {
  class A0 {
    v() {
      return 1
    }
  }
  class A1 {
    a() {
      return new A0()
    }
    b() {
      return new A0()
    }
  }
  class A2 {
    a() {
      return new A1()
    }
    b() {
      return new A1()
    }
  }
  class A3 {
    a() {
      return new A2()
    }
    b() {
      return new A2()
    }
  }
  class A4 {
    a() {
      return new A3()
    }
    b() {
      return new A3()
    }
  }
  class A5 {
    a() {
      return new A4()
    }
    b() {
      return new A4()
    }
  }
  class A6 {
    a() {
      return new A5()
    }
    b() {
      return new A5()
    }
  }
  class A7 {
    a() {
      return new A6()
    }
    b() {
      return new A6()
    }
  }
  class A8 {
    a() {
      return new A7()
    }
    b() {
      return new A7()
    }
  }
  class A9 {
    a() {
      return new A8()
    }
    b() {
      return new A8()
    }
  }
  class A10 {
    a() {
      return new A9()
    }
    b() {
      return new A9()
    }
  }
  class A11 {
    a() {
      return new A10()
    }
    b() {
      return new A10()
    }
  }
  class A12 {
    a() {
      return new A11()
    }
    b() {
      return new A11()
    }
  }
  class A13 {
    a() {
      return new A12()
    }
    b() {
      return new A12()
    }
  }
  class A14 {
    a() {
      return new A13()
    }
    b() {
      return new A13()
    }
  }
  class A15 {
    a() {
      return new A14()
    }
    b() {
      return new A14()
    }
  }
  class A16 {
    a() {
      return new A15()
    }
    b() {
      return new A15()
    }
  }
  class A17 {
    a() {
      return new A16()
    }
    b() {
      return new A16()
    }
  }
  class A18 {
    a() {
      return new A17()
    }
    b() {
      return new A17()
    }
  }
  class A19 {
    a() {
      return new A18()
    }
    b() {
      return new A18()
    }
  }
  class A20 {
    a() {
      return new A19()
    }
    b() {
      return new A19()
    }
  }
  class A21 {
    a() {
      return new A20()
    }
    b() {
      return new A20()
    }
  }
  class A22 {
    a() {
      return new A21()
    }
    b() {
      return new A21()
    }
  }
  class A23 {
    a() {
      return new A22()
    }
    b() {
      return new A22()
    }
  }
  class A24 {
    a() {
      return new A23()
    }
    b() {
      return new A23()
    }
  }
  class A25 {
    a() {
      return new A24()
    }
    b() {
      return new A24()
    }
  }
  class A26 {
    a() {
      return new A25()
    }
    b() {
      return new A25()
    }
  }
  class A27 {
    a() {
      return new A26()
    }
    b() {
      return new A26()
    }
  }
  class A28 {
    a() {
      return new A27()
    }
    b() {
      return new A27()
    }
  }
  class A29 {
    a() {
      return new A28()
    }
    b() {
      return new A28()
    }
  }
  class A30 {
    a() {
      return new A29()
    }
    b() {
      return new A29()
    }
  }
  use(new A30())
})

test('two', () => {
  class A0 {
    v() {
      return 1
    }
  }
  class A1 {
    a() {
      return new A0()
    }
    b() {
      return new A0()
    }
  }
  class A2 {
    a() {
      return new A1()
    }
    b() {
      return new A1()
    }
  }
  class A3 {
    a() {
      return new A2()
    }
    b() {
      return new A2()
    }
  }
  class A4 {
    a() {
      return new A3()
    }
    b() {
      return new A3()
    }
  }
  class A5 {
    a() {
      return new A4()
    }
    b() {
      return new A4()
    }
  }
  class A6 {
    a() {
      return new A5()
    }
    b() {
      return new A5()
    }
  }
  class A7 {
    a() {
      return new A6()
    }
    b() {
      return new A6()
    }
  }
  class A8 {
    a() {
      return new A7()
    }
    b() {
      return new A7()
    }
  }
  class A9 {
    a() {
      return new A8()
    }
    b() {
      return new A8()
    }
  }
  class A10 {
    a() {
      return new A9()
    }
    b() {
      return new A9()
    }
  }
  class A11 {
    a() {
      return new A10()
    }
    b() {
      return new A10()
    }
  }
  class A12 {
    a() {
      return new A11()
    }
    b() {
      return new A11()
    }
  }
  class A13 {
    a() {
      return new A12()
    }
    b() {
      return new A12()
    }
  }
  class A14 {
    a() {
      return new A13()
    }
    b() {
      return new A13()
    }
  }
  class A15 {
    a() {
      return new A14()
    }
    b() {
      return new A14()
    }
  }
  class A16 {
    a() {
      return new A15()
    }
    b() {
      return new A15()
    }
  }
  class A17 {
    a() {
      return new A16()
    }
    b() {
      return new A16()
    }
  }
  class A18 {
    a() {
      return new A17()
    }
    b() {
      return new A17()
    }
  }
  class A19 {
    a() {
      return new A18()
    }
    b() {
      return new A18()
    }
  }
  class A20 {
    a() {
      return new A19()
    }
    b() {
      return new A19()
    }
  }
  class A21 {
    a() {
      return new A20()
    }
    b() {
      return new A20()
    }
  }
  class A22 {
    a() {
      return new A21()
    }
    b() {
      return new A21()
    }
  }
  class A23 {
    a() {
      return new A22()
    }
    b() {
      return new A22()
    }
  }
  class A24 {
    a() {
      return new A23()
    }
    b() {
      return new A23()
    }
  }
  class A25 {
    a() {
      return new A24()
    }
    b() {
      return new A24()
    }
  }
  class A26 {
    a() {
      return new A25()
    }
    b() {
      return new A25()
    }
  }
  class A27 {
    a() {
      return new A26()
    }
    b() {
      return new A26()
    }
  }
  class A28 {
    a() {
      return new A27()
    }
    b() {
      return new A27()
    }
  }
  class A29 {
    a() {
      return new A28()
    }
    b() {
      return new A28()
    }
  }
  class A30 {
    a() {
      return new A29()
    }
    b() {
      return new A29()
    }
  }
  use(new A30())
})
