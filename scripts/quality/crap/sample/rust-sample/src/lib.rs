// Tiny sample crate for CRAP self-test.
// `simple_add` is low-complexity. `gnarly` is high cyclomatic-complexity and
// (deliberately) uncovered, so it should score CRAP > 30.

pub fn simple_add(a: i32, b: i32) -> i32 {
    a + b
}

#[allow(clippy::all)]
pub fn gnarly(x: i32, y: i32, mode: i32) -> i32 {
    let mut acc = 0;
    if x > 0 {
        if y > 0 {
            acc += x * y;
        } else if y < -10 {
            acc -= x;
        } else {
            acc += 1;
        }
    } else if x < -5 {
        match mode {
            0 => acc += 1,
            1 => acc += 2,
            2 => acc += 3,
            3 => acc += 4,
            _ => acc -= 1,
        }
    }
    for i in 0..x.abs() {
        if i % 2 == 0 && i % 3 == 0 {
            acc += i;
        } else if i % 5 == 0 || i % 7 == 0 {
            acc -= i;
        }
    }
    while acc > 100 {
        acc /= 2;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_add() {
        assert_eq!(simple_add(2, 3), 5);
        assert_eq!(simple_add(-1, 1), 0);
    }
}
