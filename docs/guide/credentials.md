# Encrypted Credentials

Autumn ships a built-in encrypted credentials store that lets you commit secrets
to your repository safely.  Secrets are encrypted at rest with AES-256-GCM; the
only thing that must stay off disk (or be injected via an environment variable)
is the master key.

---

## Quick start

```bash
# 1. Edit (or create) your development credentials
autumn credentials edit

# 2. Set a real value inside the editor, save, and quit
# The file is encrypted and saved to config/credentials/development.toml.enc

# 3. Read the credential from application code
# config.credentials().get::<String>("stripe_secret_key")
```

---

## File layout

```
my-app/
├── config/
│   ├── master.key                        ← 64-char hex key (never commit!)
│   └── credentials/
│       ├── development.toml.enc          ← encrypted dev secrets (safe to commit)
│       └── production.toml.enc           ← encrypted prod secrets (safe to commit)
└── .gitignore                            ← already excludes config/master.key
```

`autumn new` scaffolds `config/credentials/development.toml.enc` and
`config/master.key` automatically.  The `.gitignore` that is generated already
excludes `config/master.key` and includes the `.enc` files — commit the
encrypted files freely.

---

## Master key resolution

The framework resolves the master key in this order (first found wins):

| Priority | Source | Notes |
|----------|--------|-------|
| 1 | `AUTUMN_MASTER_KEY` env var | Recommended for CI / production |
| 2 | `config/master.key` file | Recommended for local development |
| – | _(none found)_ | Boot fails with an actionable error |

If the key is found but decryption fails (e.g., wrong key or corrupted file),
the error message distinguishes between the two cases and names the source that
was used so you can diagnose the problem immediately.

```
credentials error: no master key found;
  tried: AUTUMN_MASTER_KEY env var, config/master.key file

credentials error: decryption failed using key from config/master.key file (…):
  invalid key or corrupted ciphertext
```

---

## Encrypted file format

Every `.enc` file uses the following binary layout:

```
┌─────────────┬─────────────────┬─────────────────────────────┐
│ version (1B)│   nonce (12 B)  │  ciphertext + GCM tag (N+16)│
└─────────────┴─────────────────┴─────────────────────────────┘
  0x01          random per write   AES-256-GCM authenticated ciphertext
```

- **Version byte** `0x01` — reserved for future algorithm agility.
- **Nonce** — 12 bytes generated fresh from the OS CSPRNG on every write,
  ensuring that two encryptions of the same plaintext produce different
  ciphertexts.
- **Ciphertext + tag** — AES-256-GCM output; the 16-byte authentication tag is
  appended by the library.

The master key is 32 bytes (256 bits) stored as 64 lowercase hex characters.

### Roundtrip portability

Encrypt on host A, decrypt on host B — as long as both use the same master key,
the file is portable across operating systems, CPU architectures, and Autumn
versions that support format version `0x01`.

---

## CLI reference

### `autumn credentials edit [--env <env>]`

Decrypts `config/credentials/<env>.toml.enc`, opens the plaintext in your
`$VISUAL` / `$EDITOR` (falling back to `vi` on Unix, `notepad` on Windows),
re-encrypts on save with an atomic write (`*.enc.tmp` rename), and zeroes the
plaintext temp file before removing it.

```bash
autumn credentials edit                    # edits development (default)
autumn credentials edit --env production   # edits production
```

When the credentials file does not yet exist, the editor opens a template with
placeholder comments.  On first save a new master key is generated and written
to `config/master.key`.

### `autumn credentials show [--env <env>] [--reveal]`

Prints a redacted summary of the decrypted credentials (keys only, values
replaced by `[REDACTED]`).  Pass `--reveal` to print the full plaintext.

```bash
autumn credentials show                    # redacted key list
autumn credentials show --reveal           # full decrypted TOML
autumn credentials show --env production --reveal
```

---

## Reading credentials at runtime

At application boot, Autumn automatically loads
`config/credentials/<profile>.toml.enc` and makes it available on the
`AutumnConfig` struct:

```rust
#[get("/stripe-test")]
async fn test_stripe(config: AutumnConfig) -> &'static str {
    let key: Option<String> = config.credentials().get("stripe_secret_key");
    // use key …
    "ok"
}
```

The typed `get::<T>()` method deserializes any top-level TOML value into `T`.
Values are **never** emitted to logs or actuator output; the `Debug` impl for
`MasterKey` redacts the key bytes.

---

## Production deployment

For production, set the master key as an environment variable:

```bash
# Fly.io
fly secrets set AUTUMN_MASTER_KEY=$(cat config/master.key)

# Docker / Kubernetes
docker run -e AUTUMN_MASTER_KEY="<hex key>" my-app

# .env file (never commit!)
AUTUMN_MASTER_KEY=<hex key>
```

Commit `config/credentials/production.toml.enc` to the repository.  The
encrypted file is safe to store in version control — without the master key it
is indistinguishable from random bytes.

---

## Key rotation

1. Create a new master key: `openssl rand -hex 32 > config/master.key.new`
2. Decrypt the old file: `autumn credentials show --reveal > /tmp/plain.toml`
3. Re-encrypt: `AUTUMN_MASTER_KEY=$(cat config/master.key.new) autumn credentials edit`
   (paste from `/tmp/plain.toml`, save)
4. Shred the temp file: `shred -u /tmp/plain.toml`
5. Replace the old key: `mv config/master.key.new config/master.key`
6. Update the key in production secrets.

---

## Existing apps

The credentials feature is **purely additive**.  Apps that have no
`config/credentials/` directory continue to boot unchanged; the
`credentials()` accessor simply returns an empty store.
