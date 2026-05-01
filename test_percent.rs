use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'\\');

fn encode_key_path1(key: &str) -> String {
    key.split('/')
        .map(|segment| utf8_percent_encode(segment, PATH_SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_key_path2(key: &str) -> String {
    let mut result = String::with_capacity(key.len() * 2);
    let mut first = true;
    for segment in key.split('/') {
        if !first {
            result.push('/');
        }
        first = false;
        result.push_str(&utf8_percent_encode(segment, PATH_SEGMENT).to_string());
    }
    result
}

fn encode_key_path3(key: &str) -> String {
    // utf8_percent_encode yields an Iterator of &str, so we can directly collect it.
    // wait we can just iterate over split and extend.
    let mut result = String::with_capacity(key.len() + 16);
    let mut first = true;
    for segment in key.split('/') {
        if !first {
            result.push('/');
        }
        first = false;
        result.extend(utf8_percent_encode(segment, PATH_SEGMENT));
    }
    result
}


fn main() {
    let key = "a/b/c d?e";
    println!("{}", encode_key_path1(key));
    println!("{}", encode_key_path2(key));
    println!("{}", encode_key_path3(key));
}
