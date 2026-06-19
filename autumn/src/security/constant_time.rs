use subtle::{Choice, ConstantTimeEq};

/// Constant-time string comparison that does not short-circuit on length mismatch.
///
/// Ensures execution time is determined solely by the length of `a` (the trusted server value).
pub fn constant_time_eq_str(a: &str, b: &str) -> bool {
    constant_time_eq(a.as_bytes(), b.as_bytes())
}

/// Constant-time byte slice comparison that does not short-circuit on length mismatch.
///
/// Ensures execution time is determined solely by the length of `a` (the trusted server value).
#[inline(never)]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let len_eq = a.len().ct_eq(&b.len());
    let mut bytes_eq = Choice::from(1u8);
    for (i, &a_byte) in a.iter().enumerate() {
        let b_byte = *b.get(i).unwrap_or(&0xFF);
        bytes_eq &= a_byte.ct_eq(&b_byte);
    }
    (len_eq & bytes_eq).into()
}
