//! systemd unit rendering.

use std::fmt::Write;

use crate::config::{Config, SecretsMode, ServiceScope, secrets_remote_file};

/// Renders the full unit text for one host's root. The output references
/// only `{root}/current`, never a concrete release, so the unit changes
/// when the config changes and stays byte-identical across routine
/// deploys.
pub fn render(config: &Config, root: &str) -> String {
    let exec_start = config
        .service
        .exec_start
        .replace("{current}", &format!("{root}/current"));
    let mut unit = String::new();
    let _ = writeln!(unit, "[Unit]");
    let _ = writeln!(unit, "Description={} (deployed by slipway)", config.app);
    let _ = writeln!(unit, "After=network-online.target");
    let _ = writeln!(unit, "Wants=network-online.target");
    let _ = writeln!(unit);
    let _ = writeln!(unit, "[Service]");
    let _ = writeln!(unit, "Type=simple");
    let _ = writeln!(unit, "ExecStart={exec_start}");
    let _ = writeln!(unit, "WorkingDirectory={root}/current");
    if let Some(user) = &config.service.user {
        let _ = writeln!(unit, "User={user}");
    }
    for (key, value) in &config.service.env {
        let _ = writeln!(unit, "Environment=\"{key}={value}\"");
    }
    if let Some(secrets) = &config.secrets {
        let path = secrets_remote_file(root, secrets.mode);
        let _ = match secrets.mode {
            SecretsMode::EnvFile => writeln!(unit, "EnvironmentFile={path}"),
            SecretsMode::Credential => writeln!(unit, "LoadCredential=secrets.env:{path}"),
            SecretsMode::EncryptedCredential => {
                writeln!(unit, "LoadCredentialEncrypted=secrets.env:{path}")
            }
        };
    }
    for extra in &config.service.unit_extra {
        let _ = writeln!(unit, "{extra}");
    }
    let _ = writeln!(unit, "Restart=on-failure");
    let _ = writeln!(unit, "RestartSec=2");
    let _ = writeln!(unit);
    let _ = writeln!(unit, "[Install]");
    let wanted_by = match config.service.scope {
        ServiceScope::System => "multi-user.target",
        ServiceScope::User => "default.target",
    };
    let _ = writeln!(unit, "WantedBy={wanted_by}");
    unit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn renders_a_full_config() {
        let config = config(
            r#"
            app = "demo"
            hosts = ["h1"]
            [build]
            artifact = "out/demo"
            [remote]
            root = "/opt/demo"
            [service]
            exec_start = "{current}/demo --port 8080"
            user = "demo"
            env = { B_VAR = "two", A_VAR = "one" }
            unit_extra = ["LimitNOFILE=65536"]
        "#,
        );
        let expected = "\
[Unit]
Description=demo (deployed by slipway)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/demo/current/demo --port 8080
WorkingDirectory=/opt/demo/current
User=demo
Environment=\"A_VAR=one\"
Environment=\"B_VAR=two\"
LimitNOFILE=65536
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
";
        assert_eq!(render(&config, "/opt/demo"), expected);
    }

    #[test]
    fn renders_a_minimal_config() {
        let config = config(
            r#"
            app = "demo"
            hosts = ["h1"]
            [build]
            artifact = "out/demo"
            [service]
            exec_start = "{current}/demo"
        "#,
        );
        let rendered = render(&config, "/srv/demo");
        assert!(rendered.contains("ExecStart=/srv/demo/current/demo\n"));
        assert!(!rendered.contains("User="));
        assert!(!rendered.contains("Environment="));
    }

    #[test]
    fn user_scope_wants_the_default_target() {
        let config = config(
            r#"
            app = "demo"
            hosts = ["h1"]
            [build]
            artifact = "out/demo"
            [service]
            exec_start = "{current}/demo"
            scope = "user"
        "#,
        );
        let rendered = render(&config, "/srv/demo");
        assert!(rendered.contains("WantedBy=default.target\n"));
        assert!(!rendered.contains("multi-user.target"));
    }

    #[test]
    fn renders_the_secrets_line_per_mode() {
        let base = r#"
            app = "demo"
            hosts = ["h1"]
            [build]
            artifact = "out/demo"
            [service]
            exec_start = "{current}/demo"
            [secrets]
        "#;
        let env_file = render(&config(base), "/srv/demo");
        assert!(env_file.contains("EnvironmentFile=/srv/demo/shared/secrets.env\n"));

        let credential = render(
            &config(&format!("{base}mode = \"credential\"\n")),
            "/srv/demo",
        );
        assert!(credential.contains("LoadCredential=secrets.env:/srv/demo/shared/secrets.env\n"));
        assert!(!credential.contains("EnvironmentFile="));

        let encrypted = render(
            &config(&format!("{base}mode = \"encrypted-credential\"\n")),
            "/srv/demo",
        );
        assert!(
            encrypted.contains(
                "LoadCredentialEncrypted=secrets.env:/srv/demo/shared/secrets.env.cred\n"
            )
        );
    }
}
