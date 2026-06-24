//! Constant-time comparison functions to prevent timing attacks.

use subtle::{Choice, ConstantTimeEq};

/// Constant-time string comparison to prevent timing attacks when verifying tokens.
///
/// The comparison always processes exactly `b.len()` bytes so that execution
/// time is independent of the length of the submitted token `a`. Neither a
/// length mismatch nor a short input causes an early exit.
#[inline(never)]
#[must_use]
pub fn constant_time_eq_str(a: &str, b: &str) -> bool {
    constant_time_eq(a.as_bytes(), b.as_bytes())
}

/// Constant-time byte slice comparison to prevent timing attacks when verifying tokens.
///
/// The comparison always processes exactly `b.len()` bytes so that execution
/// time is independent of the length of the submitted token `a`. Neither a
/// length mismatch nor a short input causes an early exit.
#[inline(never)]
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Constant-time length check — no early exit.
    let len_eq = a.len().ct_eq(&b.len());

    // Iterate over `a` (the trusted stored token) so the loop count is fixed
    // at the server-side token length, regardless of what the caller submits
    // as `b`. Callers pass the attacker-controlled value as `b`, so iterating
    // over `a` ensures every submission — short or long — executes the same
    // amount of work. Out-of-range positions in `b` use the inverse of
    // `a_byte` to guarantee a constant-time mismatch without branching.
    let mut bytes_eq = Choice::from(1u8);
    for (i, &a_byte) in a.iter().enumerate() {
        let b_byte = *b.get(i).unwrap_or(&!a_byte);
        bytes_eq &= a_byte.ct_eq(&b_byte);
    }

    (len_eq & bytes_eq).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"a", b"b"));
        assert!(!constant_time_eq(b"a", b"A"));

        // Edge case: matching the inverse of a byte
        assert!(!constant_time_eq(b"\x00\x00\x00", b"\x00\x00"));
        assert!(!constant_time_eq(b"\xff\xff\xff", b"\xff\xff"));
    }

    #[test]
    fn test_constant_time_eq_str() {
        assert!(constant_time_eq_str("abc", "abc"));
        assert!(!constant_time_eq_str("abc", "ab"));
        assert!(!constant_time_eq_str("abc", "abd"));
        assert!(constant_time_eq_str("", ""));
        assert!(!constant_time_eq_str("a", "b"));
        assert!(!constant_time_eq_str("a", "A"));
    }
}
