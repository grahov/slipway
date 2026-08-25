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
    pub hosts: Vec<String>,
    pub build: Build,
    #[serde(default)]
    pub remote: Remote,
    pub service: Service,
    pub healthcheck: Option<Healthcheck>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub command: Option<String>,
    pub artifact: String,
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
    pub user: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub unit_extra: Vec<String>,
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

[service]
# {current} expands to the `current` symlink, e.g. /srv/myapp/current.
exec_start = "{current}/myapp"
# user = "myapp"
# env = { RUST_LOG = "info" }
# unit_extra = ["LimitNOFILE=65536"]

[healthcheck]
# Runs on the host; the deploy rolls back when it keeps failing.
command = "curl -fsS http://127.0.0.1:8080/health"
# retries = 5
# delay_ms = 1000
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
        for host in &self.hosts {
            if host.is_empty() || host.starts_with('-') || host.chars().any(char::is_whitespace) {
                bail!("host {host:?} is not a valid ssh destination");
            }
        }
        let root = self.root();
        if !root.starts_with('/') || !root.chars().all(is_path_char) {
            bail!("remote.root must be absolute and use only [A-Za-z0-9/._-], got {root:?}");
        }
        if self.remote.keep_releases == 0 {
            bail!("remote.keep_releases must be at least 1");
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
}
