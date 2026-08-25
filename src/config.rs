//! slipway.toml: model, loading, validation.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The whole config file. Anything beyond type shape is enforced by
/// [`Config::validate`], which every load path runs, so a `Config` in hand
/// is always safe to interpolate into remote command lines.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub app: String,
    pub hosts: Vec<HostEntry>,
    pub build: Build,
    #[serde(default)]
    pub remote: Remote,
    #[serde(default)]
    pub rollout: Rollout,
    pub service: Service,
    pub healthcheck: Option<Healthcheck>,
    pub secrets: Option<Secrets>,
}

/// How the fleet is walked during a deploy. Rollback, status, and secrets
/// push stay sequential regardless.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Rollout {
    /// Hosts deployed concurrently within one wave.
    pub parallel: usize,
    /// Hosts that must fully succeed before the rest of the fleet starts.
    pub canary: usize,
}

impl Default for Rollout {
    fn default() -> Self {
        Self {
            parallel: 1,
            canary: 0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub command: Option<String>,
    pub artifact: String,
}

/// A host is either a bare ssh destination or a table refining it.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum HostEntry {
    Addr(String),
    Table(HostTable),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostTable {
    pub addr: String,
    pub group: Option<String>,
    pub sudo: Option<bool>,
    pub root: Option<String>,
}

/// One host with every default resolved; the only host shape the
/// pipelines ever see.
#[derive(Debug, Clone)]
pub struct Target {
    pub addr: String,
    pub group: Option<String>,
    pub sudo: bool,
    pub root: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Remote {
    pub root: Option<String>,
    pub keep_releases: usize,
    pub sudo: bool,
}

impl Default for Remote {
    fn default() -> Self {
        Self {
            root: None,
            keep_releases: 5,
            sudo: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    /// Command line for `ExecStart=`; the `{current}` placeholder expands
    /// to `{root}/current` at render time.
    pub exec_start: String,
    #[serde(default)]
    pub scope: ServiceScope,
    pub user: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub unit_extra: Vec<String>,
}

/// Which systemd manager owns the service.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceScope {
    /// PID 1: unit in `/etc/systemd/system`, needs sudo or root.
    #[default]
    System,
    /// The ssh user's manager: unit in `~/.config/systemd/user`, no sudo
    /// anywhere; survives logout only with `loginctl enable-linger`.
    User,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Healthcheck {
    /// Runs on the host after every restart; exit 0 means healthy.
    pub command: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_delay_ms")]
    pub delay_ms: u64,
}

fn default_retries() -> u32 {
    5
}

fn default_delay_ms() -> u64 {
    1000
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Secrets {
    /// age-encrypted dotenv file, committed next to the code.
    pub file: String,
    /// Local age identity file used to decrypt at deploy time; a leading
    /// `~/` expands via `$HOME`. The identity never leaves this machine.
    pub identity: String,
    /// age public keys allowed to re-encrypt with `secrets edit`.
    pub recipients: Vec<String>,
    pub mode: SecretsMode,
}

impl Default for Secrets {
    fn default() -> Self {
        Self {
            file: "secrets.env.age".into(),
            identity: "~/.config/slipway/identity.txt".into(),
            recipients: Vec::new(),
            mode: SecretsMode::EnvFile,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretsMode {
    /// `EnvironmentFile=` in the unit: variables land in the process
    /// environment, no application changes needed.
    EnvFile,
    /// `LoadCredential=` in the unit: the application reads the file at
    /// `$CREDENTIALS_DIRECTORY/secrets.env` itself.
    Credential,
}

/// The commented template written by `slipway init`.
pub const EXAMPLE: &str = r#"# slipway deploys this app to every host listed below.
app = "myapp"
hosts = ["deploy@app1.example.com"]

[build]
# Runs locally, once per deploy, via `sh -c`. Remove to deploy a prebuilt artifact.
command = "cargo build --release"
# File or directory that must exist after the build; uploaded as the release.
artifact = "target/release/myapp"

[remote]
# root = "/srv/myapp"        # default: /srv/{app}
# keep_releases = 5          # releases kept on the host after a deploy
# sudo = true                # prefix systemctl and unit install with sudo -n

# [rollout]
# canary = 1                 # hosts that must succeed before the rest start
# parallel = 4               # hosts deployed concurrently within a wave

[service]
# {current} expands to the `current` symlink, e.g. /srv/myapp/current.
exec_start = "{current}/myapp"
# scope = "system"            # or "user": sudo-less, needs loginctl enable-linger
# user = "myapp"
# env = { RUST_LOG = "info" }
# unit_extra = ["LimitNOFILE=65536"]

[healthcheck]
# Runs on the host; the deploy rolls back when it keeps failing.
command = "curl -fsS http://127.0.0.1:8080/health"
# retries = 5
# delay_ms = 1000

# [secrets]
# Age-encrypted env file, committed to the repo. `slipway secrets init`
# creates the identity and prints the public key to put into recipients.
# file = "secrets.env.age"
# identity = "~/.config/slipway/identity.txt"
# recipients = ["age1..."]
# mode = "env-file"            # or "credential"
"#;

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn root(&self) -> String {
        match &self.remote.root {
            Some(root) => root.clone(),
            None => format!("/srv/{}", self.app),
        }
    }

    pub fn unit_name(&self) -> String {
        format!("{}.service", self.app)
    }

    /// Hosts with `remote.*` defaults applied and table overrides on top.
    pub fn targets(&self) -> Vec<Target> {
        self.hosts
            .iter()
            .map(|entry| match entry {
                HostEntry::Addr(addr) => Target {
                    addr: addr.clone(),
                    group: None,
                    sudo: self.remote.sudo,
                    root: self.root(),
                },
                HostEntry::Table(table) => Target {
                    addr: table.addr.clone(),
                    group: table.group.clone(),
                    sudo: table.sudo.unwrap_or(self.remote.sudo),
                    root: table.root.clone().unwrap_or_else(|| self.root()),
                },
            })
            .collect()
    }

    /// Rejects values that could break out of the remote command lines and
    /// unit file they are interpolated into. Error messages name the field.
    fn validate(&self) -> Result<()> {
        if self.app.is_empty() || !self.app.chars().all(is_name_char) {
            bail!(
                "app must be non-empty and use only [a-z0-9._-], got {:?}",
                self.app
            );
        }
        if self.hosts.is_empty() {
            bail!("hosts must list at least one ssh destination");
        }
        for target in self.targets() {
            let addr = &target.addr;
            if addr.is_empty() || addr.starts_with('-') || addr.chars().any(char::is_whitespace) {
                bail!("host {addr:?} is not a valid ssh destination");
            }
            let root = &target.root;
            if !root.starts_with('/') || !root.chars().all(is_path_char) {
                bail!(
                    "root for {addr} must be absolute and use only [A-Za-z0-9/._-], got {root:?}"
                );
            }
        }
        if self.remote.keep_releases == 0 {
            bail!("remote.keep_releases must be at least 1");
        }
        if self.rollout.parallel == 0 {
            bail!("rollout.parallel must be at least 1");
        }
        if self.build.artifact.is_empty() {
            bail!("build.artifact must not be empty");
        }
        if self.service.exec_start.is_empty() {
            bail!("service.exec_start must not be empty");
        }
        for (line, source) in self.unit_lines() {
            if line.contains('\n') {
                bail!("{source} must not contain newlines, got {line:?}");
            }
        }
        if let Some(user) = &self.service.user {
            if user.is_empty() || !user.chars().all(is_name_char) {
                bail!("service.user must use only [a-z0-9._-], got {user:?}");
            }
        }
        for value in self.service.env.values() {
            if value.contains('"') {
                bail!("service.env values must not contain double quotes, got {value:?}");
            }
        }
        if let Some(hc) = &self.healthcheck {
            if hc.command.is_empty() {
                bail!("healthcheck.command must not be empty");
            }
            if hc.retries == 0 {
                bail!("healthcheck.retries must be at least 1");
            }
        }
        if let Some(secrets) = &self.secrets {
            if secrets.file.is_empty() || secrets.identity.is_empty() {
                bail!("secrets.file and secrets.identity must not be empty");
            }
            for recipient in &secrets.recipients {
                if recipient.parse::<age::x25519::Recipient>().is_err() {
                    bail!("secrets.recipients entry {recipient:?} is not an age public key");
                }
            }
        }
        Ok(())
    }

    fn unit_lines(&self) -> Vec<(String, &'static str)> {
        let mut lines = vec![(self.service.exec_start.clone(), "service.exec_start")];
        for value in self.service.env.values() {
            lines.push((value.clone(), "service.env values"));
        }
        for extra in &self.service.unit_extra {
            lines.push((extra.clone(), "service.unit_extra lines"));
        }
        lines
    }
}

/// The one remote location for decrypted secrets under a host's root;
/// the deploy pipeline and the unit renderer must agree on it.
pub fn secrets_path(root: &str) -> String {
    format!("{root}/shared/secrets.env")
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    const MINIMAL: &str = r#"
        app = "demo"
        hosts = ["deploy@h1"]
        [build]
        artifact = "out/demo"
        [service]
        exec_start = "{current}/demo"
    "#;

    #[test]
    fn example_template_parses() {
        let config = parse(EXAMPLE).unwrap();
        assert_eq!(config.app, "myapp");
        assert_eq!(config.healthcheck.unwrap().retries, 5);
    }

    #[test]
    fn minimal_config_gets_defaults() {
        let config = parse(MINIMAL).unwrap();
        assert_eq!(config.root(), "/srv/demo");
        assert_eq!(config.unit_name(), "demo.service");
        assert_eq!(config.remote.keep_releases, 5);
        assert!(config.remote.sudo);
        assert!(config.build.command.is_none());
        assert!(config.healthcheck.is_none());
    }

    #[test]
    fn rejects_unsafe_values() {
        let cases = [
            ("app = \"demo\"", "app = \"De mo\""),
            ("hosts = [\"deploy@h1\"]", "hosts = []"),
            ("hosts = [\"deploy@h1\"]", "hosts = [\"-oProxyCommand=x\"]"),
            ("[build]", "[remote]\nroot = \"relative/path\"\n[build]"),
            ("[build]", "[remote]\nkeep_releases = 0\n[build]"),
        ];
        for (from, to) in cases {
            let text = MINIMAL.replace(from, to);
            assert!(parse(&text).is_err(), "expected rejection after {to:?}");
        }
    }

    #[test]
    fn rejects_unknown_fields() {
        let text = MINIMAL.replace("[build]", "typo = 1\n[build]");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn host_tables_resolve_defaults_and_overrides() {
        let text = r#"
            app = "demo"
            hosts = [
                "deploy@plain",
                { addr = "deploy@web1", group = "web" },
                { addr = "root@db1", group = "db", sudo = false, root = "/opt/demo" },
            ]
            [build]
            artifact = "out/demo"
            [service]
            exec_start = "{current}/demo"
        "#;
        let config = parse(text).unwrap();
        let targets = config.targets();
        assert_eq!(targets.len(), 3);
        assert!(targets[0].sudo && targets[0].group.is_none());
        assert_eq!(targets[0].root, "/srv/demo");
        assert_eq!(targets[1].group.as_deref(), Some("web"));
        assert!(!targets[2].sudo);
        assert_eq!(targets[2].root, "/opt/demo");

        let bad = text.replace("root = \"/opt/demo\"", "root = \"opt/demo\"");
        assert!(parse(&bad).is_err());
        let unknown = text.replace("group = \"db\", ", "grp = \"db\", ");
        assert!(parse(&unknown).is_err());
    }

    #[test]
    fn secrets_table_gets_defaults_and_rejects_bad_values() {
        let with_secrets = format!("{MINIMAL}\n[secrets]\n");
        let config = parse(&with_secrets).unwrap();
        assert_eq!(secrets_path(&config.root()), "/srv/demo/shared/secrets.env");
        let secrets = config.secrets.unwrap();
        assert_eq!(secrets.file, "secrets.env.age");
        assert_eq!(secrets.mode, SecretsMode::EnvFile);

        let bad_mode = format!("{MINIMAL}\n[secrets]\nmode = \"tpm\"\n");
        assert!(parse(&bad_mode).is_err());
        let bad_recipient = format!("{MINIMAL}\n[secrets]\nrecipients = [\"bob\"]\n");
        assert!(parse(&bad_recipient).is_err());
    }
}
