use super::BlobStoreError;

/// Validates a blob key for security and portability.
///
/// Blob keys must be lowercase, relative paths without directory traversal
/// components (`..`), Windows-reserved names/characters, or reserved suffixes
/// like `.meta`. This ensures the key is portable across `LocalBlobStore` and
/// `S3BlobStore` backends and avoids security issues like overwriting metadata
/// files or escaping the storage root.
///
/// # Errors
///
/// Returns [`BlobStoreError::InvalidInput`] when the key is rejected.
pub fn validate_key(key: &str) -> Result<(), BlobStoreError> {
    check_basic_formatting(key)?;
    check_windows_paths(key)?;
    for segment in key.split('/') {
        validate_segment(segment)?;
    }
    check_reserved_suffixes(key)?;
    check_case_folding(key)?;
    Ok(())
}

fn check_basic_formatting(key: &str) -> Result<(), BlobStoreError> {
    if key.is_empty() {
        return Err(BlobStoreError::InvalidInput("blob key is empty".into()));
    }
    if key.contains('\0') {
        return Err(BlobStoreError::InvalidInput(
            "blob key contains NUL byte".into(),
        ));
    }
    if key.starts_with('/') || key.starts_with('\\') {
        return Err(BlobStoreError::InvalidInput(
            "blob key must be relative".into(),
        ));
    }
    if key.contains('\\') {
        return Err(BlobStoreError::InvalidInput(
            "blob key contains a backslash; use `/` as the segment separator".into(),
        ));
    }
    Ok(())
}

fn check_windows_paths(key: &str) -> Result<(), BlobStoreError> {
    let bytes = key.as_bytes();
    let drive_letter = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if drive_letter {
        return Err(BlobStoreError::InvalidInput(
            "blob key looks like a Windows drive-letter path".into(),
        ));
    }
    if key.starts_with("\\\\") || key.starts_with("//") {
        return Err(BlobStoreError::InvalidInput(
            "blob key looks like a UNC / network path".into(),
        ));
    }
    Ok(())
}

fn validate_segment(segment: &str) -> Result<(), BlobStoreError> {
    if segment == ".." {
        return Err(BlobStoreError::InvalidInput(
            "blob key contains traversal segment".into(),
        ));
    }
    if segment == "." {
        return Err(BlobStoreError::InvalidInput(
            "blob key contains self-referential segment `.`".into(),
        ));
    }
    if segment.bytes().any(|b| {
        matches!(
            b,
            b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*' | 0x01..=0x1F
        )
    }) {
        return Err(BlobStoreError::InvalidInput(
            "blob key contains a Windows-reserved filename character (`<`, `>`, \
             `:`, `\"`, `|`, `?`, `*`, or a control byte) — keys must be portable \
             across local and S3 backends"
                .into(),
        ));
    }
    let basename = segment.split('.').next().unwrap_or("");
    if WINDOWS_RESERVED_NAMES.contains(&basename) {
        return Err(BlobStoreError::InvalidInput(format!(
            "blob key segment {segment:?} starts with a Windows-reserved device name \
             ({basename:?}) and cannot be written to local disk on Windows"
        )));
    }
    Ok(())
}

fn check_reserved_suffixes(key: &str) -> Result<(), BlobStoreError> {
    if let Some(last) = key.split('/').next_back() {
        let bytes = last.as_bytes();
        if bytes.len() >= 5 && bytes[bytes.len() - 5..].eq_ignore_ascii_case(b".meta") {
            return Err(BlobStoreError::InvalidInput(
                "blob keys ending in `.meta` are reserved (local backend uses `<key>.meta` \
                 sidecar files for content-type metadata)"
                    .into(),
            ));
        }
    }
    Ok(())
}

fn check_case_folding(key: &str) -> Result<(), BlobStoreError> {
    for c in key.chars() {
        let mut lower = c.to_lowercase();
        let first = lower.next();
        let trailing = lower.next();
        if first != Some(c) || trailing.is_some() {
            return Err(BlobStoreError::InvalidInput(
                "blob keys must be lowercase (uppercase Unicode aliases on case-insensitive \
                 filesystems and breaks portability between local and S3)"
                    .into(),
            ));
        }
    }
    Ok(())
}

const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_key_accepts_typical_paths() {
        validate_key("avatars/123.png").unwrap();
        validate_key("a/b/c/d.txt").unwrap();
    }

    #[test]
    fn validate_key_rejects_traversal() {
        let err = validate_key("../etc/passwd").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn validate_key_rejects_absolute() {
        let err = validate_key("/etc/passwd").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn validate_key_rejects_empty() {
        let err = validate_key("").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn validate_key_rejects_nul() {
        let err = validate_key("a\0b").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn validate_key_rejects_windows_drive_letter() {
        for k in [r"C:\tmp\x", "C:/tmp/x", "z:\\foo", "a:bar"] {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_key_rejects_unc_paths() {
        for k in [r"\\server\share\file", "//server/share/file"] {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_key_rejects_dot_segments() {
        // `a/./b` would resolve to `a/b` on the filesystem, aliasing two
        // distinct logical keys. HTTP clients also tend to normalize
        // these out of URL paths, breaking signature verification.
        for k in ["a/./b", "./foo", "a/././b", "a/.\\b"] {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_key_rejects_empty_segments() {
        // Same aliasing/canonicalization problem as `.` segments.
        // `a//b` collapses to `a/b` on POSIX; `a/b/` produces a trailing
        // empty segment that HTTP clients silently strip.
        for k in ["a//b", "a/b/", "a///b"] {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_key_rejects_backslash_separator() {
        // Backslashes alias to forward slashes on Windows; reject them
        // entirely so the canonical separator is always `/`.
        for k in [r"a\b", r"avatars\me.png", r"x\y\z"] {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_key_reserves_meta_suffix() {
        // The local backend stores `<key>.meta` sidecars; a user key
        // ending in `.meta` would collide with another key's sidecar.
        // Case-insensitive because some filesystems normalize case.
        for k in ["foo.meta", "avatars/me.meta", "FOO.META", "x/y/Z.MeTa"] {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be reserved",
            );
        }
        // But these are fine — not the right suffix.
        for k in ["meta.png", "foo.metadata", "a.meta.gz", "metafile"] {
            validate_key(k).unwrap_or_else(|_| panic!("key {k:?} should be accepted"));
        }
    }

    #[test]
    fn validate_key_handles_non_ascii_without_panicking() {
        // The `.meta` suffix check must compare raw bytes, not a
        // `&str` slice — otherwise a non-ASCII key whose byte length
        // is ≥ 5 with a UTF-8 char boundary mid-suffix would panic
        // with "byte index N is not a char boundary". Pin that we
        // accept such keys cleanly instead.
        for k in ["ééé", "résumé.png", "東京", "cafe\u{0301}"] {
            validate_key(k).unwrap_or_else(|err| {
                panic!("non-ASCII key {k:?} should validate cleanly, got {err:?}")
            });
        }
        // Non-ASCII keys that *do* end in `.meta` must still be
        // rejected (the suffix is ASCII).
        let err = validate_key("résumé.meta").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidInput(_)));
    }

    #[test]
    fn validate_key_rejects_uppercase() {
        // Case-insensitive filesystems (NTFS, APFS) alias these with
        // their lowercase counterparts on the local backend while
        // keeping them distinct in app data. Reject up-front so the
        // portable subset (Unicode-lowercase / caseless) is the only
        // valid form. Covers ASCII uppercase + Unicode uppercase
        // (`Ä`, `É`, `İ`, etc.) — anything whose Unicode default
        // case-fold differs from itself.
        let rejected = [
            // ASCII uppercase
            "Foo.png",
            "AVATARS/me.png",
            "aBc",
            "x/Y/z",
            // Unicode uppercase variants
            "Ärger.png",
            "documents/Émile.txt",
            "İstanbul/photo.jpg",
            "ΟΛΑ.txt", // Greek Omicron-Lambda-Alpha
        ];
        for k in rejected {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected for uppercase"
            );
        }
        // Lowercase ASCII, lowercase Unicode, and caseless characters
        // stay valid.
        let accepted = [
            "foo.png",
            "avatars/me.png",
            "résumé.png",
            "ärger.png",
            "émile.txt",
            "istanbul/photo.jpg",
            "東京/photo.jpg", // CJK ideographs are caseless
            "café/menu.txt",
        ];
        for k in accepted {
            validate_key(k)
                .unwrap_or_else(|err| panic!("key {k:?} should be accepted, got {err:?}"));
        }
    }

    #[test]
    fn validate_key_rejects_windows_reserved_chars() {
        // `<`, `>`, `:`, `"`, `|`, `?`, `*` aren't allowed in Windows
        // filenames; control bytes (\x01-\x1F) likewise. Reject so the
        // local backend behaves the same on every platform.
        let rejected = [
            "foo<bar",
            "foo>bar",
            "foo:bar",
            "foo\"bar",
            "foo|bar",
            "foo?bar",
            "foo*bar",
            "foo\x01bar",
            "foo\x1fbar",
        ];
        for k in rejected {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_key_rejects_windows_reserved_names() {
        // `con.png`, `nul/foo`, `com1.txt`, etc. error with I/O on
        // Windows even with valid characters and casing. Reject the
        // entire reserved set per segment.
        let rejected = [
            "con",
            "con.png",
            "con/foo.png",
            "x/nul",
            "x/nul.txt",
            "aux.bin",
            "prn",
            "com1.log",
            "com9",
            "lpt1",
            "lpt9.txt",
        ];
        for k in rejected {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
        // Names that *contain* a reserved word but aren't equal to one
        // before the first dot stay valid.
        let accepted = [
            "console.png",
            "lptastic.txt",
            "x/auxiliary.bin",
            "con-tinuation.png",
            "com10.log", // reserved set is com1-9 only
        ];
        for k in accepted {
            validate_key(k)
                .unwrap_or_else(|err| panic!("key {k:?} should be accepted, got {err:?}"));
        }
    }

    #[test]
    fn validate_key_rejects_trailing_dot_or_space_segments() {
        // Windows strips trailing `.` and trailing space from
        // filenames, so two distinct logical keys would alias on the
        // local backend (and bypass the reserved-name guard for
        // `con ` / `con.`). Reject up-front.
        let rejected = [
            "foo.",            // trailing dot
            "avatars/me.png.", // trailing dot on last segment
            "x./y",            // trailing dot mid-path
            "foo ",            // trailing space
            "x /y",            // trailing space mid-path
            "con ",            // would alias `con` after Windows normalization
            "con.",            // same
        ];
        for k in rejected {
            let err = validate_key(k).unwrap_err();
            assert!(
                matches!(err, BlobStoreError::InvalidInput(_)),
                "key {k:?} should be rejected"
            );
        }
        // Internal/leading dots and spaces are still fine; only the
        // segment-trailing forms are forbidden.
        let accepted = ["foo.bar", "a b", " foo", "x/y/.hidden"];
        for k in accepted {
            validate_key(k)
                .unwrap_or_else(|err| panic!("key {k:?} should be accepted, got {err:?}"));
        }
    }

    #[test]
    fn error_status_mapping() {
        assert_eq!(
            BlobStoreError::NotFound("x".into()).status(),
            http::StatusCode::NOT_FOUND
        );
        assert_eq!(
            BlobStoreError::PermissionDenied("x".into()).status(),
            http::StatusCode::FORBIDDEN
        );
        assert_eq!(
            BlobStoreError::InvalidInput("x".into()).status(),
            http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            BlobStoreError::Signature("x".into()).status(),
            http::StatusCode::FORBIDDEN
        );
        assert_eq!(
            BlobStoreError::PayloadTooLarge("x".into()).status(),
            http::StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            BlobStoreError::Unsupported("x".into()).status(),
            http::StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            BlobStoreError::Backend("x".into()).status(),
            http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn error_into_autumn_error_preserves_status() {
        let err = BlobStoreError::NotFound("k".into()).into_autumn_error();
        assert_eq!(err.status(), http::StatusCode::NOT_FOUND);
    }
}
