use std::hint::black_box;
use std::time::Instant;

fn ct_eq_subtle(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

fn ct_eq_safe(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    let len_eq = a.len().ct_eq(&b.len());
    let mut bytes_eq = subtle::Choice::from(1u8);
    for (i, &a_byte) in a.iter().enumerate() {
        let b_byte = *b.get(i).unwrap_or(&0xFF);
        bytes_eq &= a_byte.ct_eq(&b_byte);
    }
    (len_eq & bytes_eq).into()
}

fn main() {
    let secret = vec![0u8; 10_000];

    // Short circuit (different length)
    let bad_len = vec![0u8; 1];

    // Full traversal (same length)
    let bad_val = vec![1u8; 10_000];

    // Warmup
    for _ in 0..1000 {
        black_box(ct_eq_subtle(black_box(&secret), black_box(&bad_len)));
        black_box(ct_eq_subtle(black_box(&secret), black_box(&bad_val)));
    }

    let mut times_len = Vec::new();
    let mut times_val = Vec::new();

    for _ in 0..10_000 {
        let t1 = Instant::now();
        black_box(ct_eq_subtle(black_box(&secret), black_box(&bad_len)));
        times_len.push(t1.elapsed().as_nanos());

        let t2 = Instant::now();
        black_box(ct_eq_subtle(black_box(&secret), black_box(&bad_val)));
        times_val.push(t2.elapsed().as_nanos());
    }

    let avg_len: u128 = times_len.iter().sum::<u128>() / times_len.len() as u128;
    let avg_val: u128 = times_val.iter().sum::<u128>() / times_val.len() as u128;

    println!("subtle::ConstantTimeEq:");
    println!("Different length avg: {} ns", avg_len);
    println!("Same length avg:      {} ns", avg_val);
}
