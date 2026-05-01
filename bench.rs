use std::time::Instant;

fn format_unguarded_repository_listing1(offenders: &[(String, String)]) -> String {
    offenders
        .iter()
        .map(|(name, path)| format!("  - #[repository({name}, api = \"{path}\")]"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_unguarded_repository_listing2(offenders: &[(String, String)]) -> String {
    if offenders.is_empty() {
        return String::new();
    }

    // estimate capacity: "  - #[repository(..., api = \"...\")]\n" is roughly 36 + name.len() + path.len()
    let mut capacity = 0;
    for (name, path) in offenders {
        capacity += 36 + name.len() + path.len();
    }

    let mut out = String::with_capacity(capacity);
    let mut first = true;
    for (name, path) in offenders {
        if !first {
            out.push('\n');
        }
        first = false;
        use std::fmt::Write;
        let _ = write!(out, "  - #[repository({name}, api = \"{path}\")]");
    }
    out
}

fn main() {
    let mut offenders = Vec::new();
    for i in 0..100 {
        offenders.push((format!("Resource{}", i), format!("/api/resource/{}", i)));
    }

    let now = Instant::now();
    for _ in 0..100000 {
        format_unguarded_repository_listing1(&offenders);
    }
    println!("format_unguarded_repository_listing1: {:?}", now.elapsed());

    let now = Instant::now();
    for _ in 0..100000 {
        format_unguarded_repository_listing2(&offenders);
    }
    println!("format_unguarded_repository_listing2: {:?}", now.elapsed());
}
