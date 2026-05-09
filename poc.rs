// AHA! "X-Forwarded-For" is parsed like this:
// let xff_ip = req.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|s| s.split(',').next()).map(str::trim)...
// `.next()` gets the FIRST entry in the list!
// But if multiple proxies are involved, the left-most IP is the one supplied by the client (untrusted),
// and the right-most IPs are appended by the trusted proxies.
// An attacker can easily spoof the left-most IP `X-Forwarded-For: SPOOFED_IP, REAL_IP`.
// This allows an attacker to bypass the rate limit by rotating the spoofed IP!
fn main() {}
