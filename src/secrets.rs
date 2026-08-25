//! App secrets: an age-encrypted dotenv file in the repo, decrypted
//! locally and installed onto hosts as part of the deploy.
//!
//! The identity stays on the workstation. Hosts receive plaintext at
//! `{root}/shared/secrets.env`, mode 0600, owned by the deploy user —
//! PID 1 reads it for both `EnvironmentFile=` and `LoadCredential=`, so
//! the service user itself needs no access. Every overwrite keeps the
//! previous file as `.prev` for the rollback paths.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Command;

use age::armor::{ArmoredReader, ArmoredWriter, Format};
use age::secrecy::ExposeSecret;
use age::x25519::{Identity, Recipient};
use anyhow::{Context, Result, bail};

use crate::config::{Config, Secrets};
use crate::ui;

/// Expands a leading `~/` via `$HOME`; other paths pass through.
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

fn load_identities(secrets: &Secrets) -> Result<Vec<Identity>> {
    let path = expand_home(&secrets.identity);
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "cannot read identity {} (run `slipway secrets init`)",
            path.display()
        )
    })?;
    let identities: Vec<Identity> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.parse::<Identity>())
        .collect::<Result<_, _>>()
        .map_err(|err| anyhow::anyhow!("{}: {err}", path.display()))?;
    if identities.is_empty() {
        bail!("{} contains no age identities", path.display());
    }
    Ok(identities)
}

fn recipients(secrets: &Secrets) -> Result<Vec<Recipient>> {
    if secrets.recipients.is_empty() {
        bail!("secrets.recipients is empty: add the public key `slipway secrets init` printed");
    }
    Ok(secrets
        .recipients
        .iter()
        .map(|r| r.parse::<Recipient>().expect("validated at config load"))
        .collect())
}

pub fn decrypt(secrets: &Secrets) -> Result<String> {
    let ciphertext =
        fs::read(&secrets.file).with_context(|| format!("cannot read {}", secrets.file))?;
    let identities = load_identities(secrets)?;
    let decryptor = age::Decryptor::new(ArmoredReader::new(&ciphertext[..]))
        .with_context(|| format!("{} is not an age file", secrets.file))?;
    let mut reader = decryptor
        .decrypt(identities.iter().map(|id| id as &dyn age::Identity))
        .with_context(|| {
            format!(
                "cannot decrypt {} with the configured identity",
                secrets.file
            )
        })?;
    let mut plaintext = String::new();
    std::io::Read::read_to_string(&mut reader, &mut plaintext)
        .with_context(|| format!("{} does not contain text", secrets.file))?;
    Ok(plaintext)
}

fn encrypt(secrets: &Secrets, plaintext: &str) -> Result<Vec<u8>> {
    let recipients = recipients(secrets)?;
    let encryptor =
        age::Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn age::Recipient))
            .context("cannot build the age encryptor")?;
    let mut ciphertext = Vec::new();
    let armor = ArmoredWriter::wrap_output(&mut ciphertext, Format::AsciiArmor)?;
    let mut writer = encryptor.wrap_output(armor)?;
    writer.write_all(plaintext.as_bytes())?;
    writer.finish()?.finish()?;
    Ok(ciphertext)
}

/// Rejects payloads systemd would misread. Comments and blank lines are
/// fine; every other line must be `KEY=...` with a `[A-Za-z_][A-Za-z0-9_]*`
/// key.
pub fn validate_dotenv(text: &str) -> Result<()> {
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key = line.split('=').next().unwrap_or_default();
        let mut chars = key.chars();
        let head_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        let tail_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !line.contains('=') || !head_ok || !tail_ok {
            bail!("secrets line {} is not a KEY=VALUE line", index + 1);
        }
    }
    Ok(())
}

/// One remote script per overwrite: back up, write via a temp file, tighten
/// the mode, move into place. The plaintext arrives on stdin and never
/// appears in a command line.
pub fn upload_script(path: &str) -> String {
    let dir = path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(path);
    format!(
        "mkdir -p {dir} && if [ -f {path} ]; then cp -p {path} {path}.prev; fi \
         && cat > {path}.tmp && chmod 600 {path}.tmp && mv -f {path}.tmp {path}"
    )
}

pub fn restore_script(path: &str) -> String {
    format!("if [ -f {path}.prev ]; then mv -f {path}.prev {path}; fi")
}

/// `slipway secrets init`: create the identity when missing, print its
/// public key, and create the encrypted empty file once recipients exist.
pub fn init(config: &Config) -> Result<()> {
    let secrets = required(config)?;
    let identity_path = expand_home(&secrets.identity);
    if identity_path.exists() {
        let public = load_identities(secrets)?[0].to_public();
        ui::note(&format!("identity exists; public key: {public}"));
    } else {
        if let Some(parent) = identity_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let identity = Identity::generate();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&identity_path)?;
        writeln!(file, "# created by slipway secrets init")?;
        writeln!(file, "# public key: {}", identity.to_public())?;
        writeln!(file, "{}", identity.to_string().expose_secret())?;
        ui::ok(&format!("wrote {}", identity_path.display()));
        ui::note(&format!("public key: {}", identity.to_public()));
    }
    if secrets.recipients.is_empty() {
        ui::note("add the public key to secrets.recipients, then rerun to create the file");
        return Ok(());
    }
    if fs::exists(&secrets.file)? {
        ui::note(&format!("{} already exists", secrets.file));
        return Ok(());
    }
    fs::write(&secrets.file, encrypt(secrets, "")?)?;
    ui::ok(&format!(
        "wrote empty {}; fill it with `slipway secrets edit`",
        secrets.file
    ));
    Ok(())
}

/// `slipway secrets edit`: decrypt to a private temp file, hand it to the
/// editor, validate, re-encrypt to every recipient.
pub fn edit(config: &Config) -> Result<()> {
    let secrets = required(config)?;
    let plaintext = if fs::exists(&secrets.file)? {
        decrypt(secrets)?
    } else {
        String::new()
    };
    let temp = std::env::temp_dir().join(format!("slipway-secrets-{}.env", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(plaintext.as_bytes())?;
    drop(file);
    let outcome = run_editor(&temp);
    let edited = outcome.and_then(|()| Ok(fs::read_to_string(&temp)?));
    fs::remove_file(&temp).ok();
    let edited = edited?;
    validate_dotenv(&edited)?;
    fs::write(&secrets.file, encrypt(secrets, &edited)?)?;
    ui::ok(&format!(
        "encrypted {} to {} recipient(s)",
        secrets.file,
        secrets.recipients.len()
    ));
    Ok(())
}

fn run_editor(path: &std::path::Path) -> Result<()> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".into());
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\"",))
        .arg("editor")
        .arg(path)
        .status()
        .with_context(|| format!("cannot run editor {editor}"))?;
    if !status.success() {
        bail!("editor exited with an error; secrets left unchanged");
    }
    Ok(())
}

pub fn required(config: &Config) -> Result<&Secrets> {
    config
        .secrets
        .as_ref()
        .context("no [secrets] table in the config")
}

#[cfg(test)]
mod tests {
    use std::iter;

    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let identity = Identity::generate();
        let secrets = Secrets {
            recipients: vec![identity.to_public().to_string()],
            ..Secrets::default()
        };
        let ciphertext = encrypt(&secrets, "API_KEY=sk-1\n").unwrap();
        assert!(ciphertext.starts_with(b"-----BEGIN AGE ENCRYPTED FILE-----"));

        let decryptor = age::Decryptor::new(ArmoredReader::new(&ciphertext[..])).unwrap();
        let mut reader = decryptor
            .decrypt(iter::once(&identity as &dyn age::Identity))
            .unwrap();
        let mut plaintext = String::new();
        std::io::Read::read_to_string(&mut reader, &mut plaintext).unwrap();
        assert_eq!(plaintext, "API_KEY=sk-1\n");
    }

    #[test]
    fn dotenv_shape_is_enforced() {
        validate_dotenv("# comment\n\nAPI_KEY=x\n_UNDER=1\nA1=with = signs\n").unwrap();
        assert!(validate_dotenv("no equals sign\n").is_err());
        assert!(validate_dotenv("1BAD=x\n").is_err());
        assert!(validate_dotenv("BAD KEY=x\n").is_err());
    }

    #[test]
    fn remote_scripts_back_up_and_restore() {
        let path = "/srv/demo/shared/secrets.env";
        assert_eq!(
            upload_script(path),
            "mkdir -p /srv/demo/shared \
             && if [ -f /srv/demo/shared/secrets.env ]; then \
cp -p /srv/demo/shared/secrets.env /srv/demo/shared/secrets.env.prev; fi \
             && cat > /srv/demo/shared/secrets.env.tmp \
             && chmod 600 /srv/demo/shared/secrets.env.tmp \
             && mv -f /srv/demo/shared/secrets.env.tmp /srv/demo/shared/secrets.env"
        );
        assert_eq!(
            restore_script(path),
            "if [ -f /srv/demo/shared/secrets.env.prev ]; then \
mv -f /srv/demo/shared/secrets.env.prev /srv/demo/shared/secrets.env; fi"
        );
    }

    #[test]
    fn home_expansion_only_touches_the_tilde_prefix() {
        unsafe { std::env::set_var("HOME", "/home/demo") };
        assert_eq!(
            expand_home("~/.config/x"),
            PathBuf::from("/home/demo/.config/x")
        );
        assert_eq!(expand_home("/abs/path"), PathBuf::from("/abs/path"));
    }
}
