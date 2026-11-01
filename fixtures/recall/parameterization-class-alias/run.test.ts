import { expect, test } from 'vitest'

test('aliases one', () => {
  class A1 {
    a() {
      return 1
    }
    b() {
      return 2
    }
  }
  class A2 {
    a() {
      return A1
    }
    b() {
      return A1
    }
  }
  class A3 {
    a() {
      return A2
    }
    b() {
      return A2
    }
  }
  class A4 {
    a() {
      return A3
    }
    b() {
      return A3
    }
  }
  class A5 {
    a() {
      return A4
    }
    b() {
      return A4
    }
  }
  class A6 {
    a() {
      return A5
    }
    b() {
      return A5
    }
  }
  class A7 {
    a() {
      return A6
    }
    b() {
      return A6
    }
  }
  class A8 {
    a() {
      return A7
    }
    b() {
      return A7
    }
  }
  class A9 {
    a() {
      return A8
    }
    b() {
      return A8
    }
  }
  class A10 {
    a() {
      return A9
    }
    b() {
      return A9
    }
  }
  const alias = A10
  expect(1).toBe(1)
})

test('aliases two', () => {
  class A1 {
    a() {
      return 1
    }
    b() {
      return 2
    }
  }
  class A2 {
    a() {
      return A1
    }
    b() {
      return A1
    }
  }
  class A3 {
    a() {
      return A2
    }
    b() {
      return A2
    }
  }
  class A4 {
    a() {
      return A3
    }
    b() {
      return A3
    }
  }
  class A5 {
    a() {
      return A4
    }
    b() {
      return A4
    }
  }
  class A6 {
    a() {
      return A5
    }
    b() {
      return A5
    }
  }
  class A7 {
    a() {
      return A6
    }
    b() {
      return A6
    }
  }
  class A8 {
    a() {
      return A7
    }
    b() {
      return A7
    }
  }
  class A9 {
    a() {
      return A8
    }
    b() {
      return A8
    }
  }
  class A10 {
    a() {
      return A9
    }
    b() {
      return A9
    }
  }
  const alias = A10
  expect(2).toBe(2)
})
