//! CLI implementation for `autumn credentials edit` and `autumn credentials show`.

use std::path::{Path, PathBuf};

use autumn_web::credentials::{
    CredentialsError, MasterKey, credentials_path, decrypt, encrypt, load_credentials,
    resolve_master_key,
};

pub struct EditOptions {
    pub env: String,
}

pub struct ShowOptions {
    pub env: String,
    pub reveal: bool,
}

/// Run `autumn credentials edit [--env <env>]`.
///
/// Resolves the master key, decrypts the credentials file into a temp file,
/// opens `$VISUAL` / `$EDITOR` (falling back to `vi` on Unix, `notepad` on
/// Windows), re-encrypts on save, then zeroes and removes the temp file.
pub fn run_edit(opts: &EditOptions) {
    let base_dir = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("autumn credentials: cannot determine current directory: {e}");
        std::process::exit(1);
    });

    let result = edit_credentials(&opts.env, &base_dir);
    if let Err(e) = result {
        eprintln!("autumn credentials edit: {e}");
        std::process::exit(1);
    }
}

fn edit_credentials(env: &str, base_dir: &Path) -> Result<(), CredentialsError> {
    use std::io::Write;

    let enc_path = credentials_path(env, base_dir);

    let plaintext = if enc_path.exists() {
        let key = resolve_master_key(base_dir).map_err(|e| {
            eprintln!("autumn credentials edit: {e}");
            e
        })?;
        let ciphertext = std::fs::read(&enc_path)?;
        decrypt(&key, &ciphertext)?
    } else {
        std::fs::create_dir_all(enc_path.parent().unwrap())?;
        default_credentials_template(env).into_bytes()
    };

    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("autumn-credentials-{env}.toml"));

    {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut f = options.open(&tmp_path)?;
        f.write_all(&plaintext)?;
    }

    let editor = resolve_editor();
    let status = launch_editor(&editor, &tmp_path)
        .map_err(|e| std::io::Error::other(format!("cannot launch editor '{editor}': {e}")))?;

    if !status.success() {
        zero_file(&tmp_path);
        let _ = std::fs::remove_file(&tmp_path);
        eprintln!("autumn credentials edit: editor exited with non-zero status");
        std::process::exit(1);
    }

    let new_plaintext = std::fs::read(&tmp_path)?;

    toml::from_str::<toml::Table>(std::str::from_utf8(&new_plaintext).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "file is not valid UTF-8")
    })?)
    .map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("TOML parse error: {e}"),
        )
    })?;

    let key = if enc_path.exists() {
        resolve_master_key(base_dir)?
    } else {
        let k = MasterKey::generate();
        let key_path = base_dir.join("config/master.key");
        std::fs::create_dir_all(key_path.parent().unwrap())?;
        std::fs::write(&key_path, k.to_hex())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }
        println!("  Created config/master.key (keep this secret, do not commit)");
        k
    };

    let ciphertext = encrypt(&key, &new_plaintext);

    let tmp_enc = enc_path.with_extension("enc.tmp");
    std::fs::write(&tmp_enc, &ciphertext)?;
    std::fs::rename(&tmp_enc, &enc_path)?;

    zero_file(&tmp_path);
    let _ = std::fs::remove_file(&tmp_path);

    println!("  Saved {}", enc_path.display());
    Ok(())
}

/// Run `autumn credentials show [--env <env>] [--reveal]`.
pub fn run_show(opts: &ShowOptions) {
    let base_dir = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("autumn credentials: cannot determine current directory: {e}");
        std::process::exit(1);
    });

    let result = show_credentials(&opts.env, opts.reveal, &base_dir);
    if let Err(e) = result {
        eprintln!("autumn credentials show: {e}");
        std::process::exit(1);
    }
}

fn show_credentials(env: &str, reveal: bool, base_dir: &Path) -> Result<(), CredentialsError> {
    let enc_path = credentials_path(env, base_dir);
    if !enc_path.exists() {
        println!("No credentials file found at {}", enc_path.display());
        return Ok(());
    }

    let store = load_credentials(env, base_dir)?;

    if reveal {
        let key = resolve_master_key(base_dir)?;
        let ciphertext = std::fs::read(&enc_path)?;
        let plaintext = decrypt(&key, &ciphertext)?;
        let s = String::from_utf8(plaintext)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "not valid UTF-8"))?;
        print!("{s}");
    } else {
        println!("# Credentials for '{env}' (values redacted; use --reveal to show)");
        for key_name in store.keys() {
            println!("{key_name} = [REDACTED]");
        }
    }

    Ok(())
}

fn resolve_editor() -> String {
    if let Ok(v) = std::env::var("VISUAL")
        && !v.is_empty()
    {
        return v;
    }
    if let Ok(v) = std::env::var("EDITOR")
        && !v.is_empty()
    {
        return v;
    }
    if cfg!(windows) {
        "notepad".to_owned()
    } else {
        "vi".to_owned()
    }
}

fn launch_editor(
    editor: &str,
    file: &std::path::Path,
) -> std::io::Result<std::process::ExitStatus> {
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");
    let extra_args: Vec<&str> = parts.collect();
    std::process::Command::new(program)
        .args(extra_args)
        .arg(file)
        .status()
}

fn zero_file(path: &PathBuf) {
    use std::io::Write;
    if let (Ok(meta), Ok(mut f)) = (
        std::fs::metadata(path),
        std::fs::OpenOptions::new().write(true).open(path),
    ) {
        let mut remaining = usize::try_from(meta.len()).unwrap_or(usize::MAX);
        let chunk = [0u8; 4096];
        while remaining > 0 {
            let n = remaining.min(chunk.len());
            if f.write_all(&chunk[..n]).is_err() {
                break;
            }
            remaining -= n;
        }
        let _ = f.flush();
    }
}

fn default_credentials_template(env: &str) -> String {
    format!(
        "# Encrypted credentials for '{env}'\n\
         # Run `autumn credentials edit --env {env}` to edit these values.\n\
         # Do NOT commit config/master.key to version control.\n\
         \n\
         # stripe_secret_key = \"sk_live_...\"\n\
         # sendgrid_api_key = \"SG...\"\n\
         # s3_access_key_id = \"AKIA...\"\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use autumn_web::credentials::{MasterKey, encrypt};
    use tempfile::TempDir;

    fn setup_credentials(tmp: &TempDir, env: &str, content: &str) -> MasterKey {
        let key = MasterKey::generate();
        let ct = encrypt(&key, content.as_bytes());
        std::fs::create_dir_all(tmp.path().join("config/credentials")).unwrap();
        std::fs::write(
            tmp.path()
                .join(format!("config/credentials/{env}.toml.enc")),
            &ct,
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join("config")).unwrap();
        std::fs::write(tmp.path().join("config/master.key"), key.to_hex()).unwrap();
        key
    }

    #[test]
    fn resolve_editor_returns_visual_if_set() {
        temp_env::with_var("VISUAL", Some("nano"), || {
            assert_eq!(resolve_editor(), "nano");
        });
    }

    #[test]
    fn resolve_editor_falls_back_to_editor_var() {
        temp_env::with_vars(
            [("VISUAL", None::<&str>), ("EDITOR", Some("emacs"))],
            || {
                assert_eq!(resolve_editor(), "emacs");
            },
        );
    }

    #[test]
    fn resolve_editor_falls_back_to_platform_default() {
        temp_env::with_vars([("VISUAL", None::<&str>), ("EDITOR", None::<&str>)], || {
            let editor = resolve_editor();
            if cfg!(windows) {
                assert_eq!(editor, "notepad");
            } else {
                assert_eq!(editor, "vi");
            }
        });
    }

    #[test]
    fn default_credentials_template_contains_env_name() {
        let t = default_credentials_template("production");
        assert!(t.contains("production"));
    }

    #[test]
    fn default_credentials_template_has_placeholder_comments() {
        let t = default_credentials_template("staging");
        assert!(t.contains("stripe_secret_key"));
        assert!(t.contains("sendgrid_api_key"));
    }

    #[test]
    fn show_credentials_redacted_when_reveal_false() {
        let tmp = TempDir::new().unwrap();
        setup_credentials(&tmp, "development", "stripe_key = \"sk_live_test\"\n");
        show_credentials("development", false, tmp.path()).unwrap();
    }

    #[test]
    fn show_credentials_no_file_does_not_error() {
        let tmp = TempDir::new().unwrap();
        show_credentials("development", false, tmp.path()).unwrap();
    }

    #[test]
    fn zero_file_overwrites_contents() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("secret.txt");
        std::fs::write(&path, b"super secret data 12345").unwrap();
        zero_file(&path);
        let after = std::fs::read(&path).unwrap();
        assert!(after.iter().all(|&b| b == 0), "file should be zeroed");
    }
}
