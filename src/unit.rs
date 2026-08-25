//! systemd unit rendering.

use std::fmt::Write;

use crate::config::Config;

/// Renders the full unit text. The output references only `{root}/current`,
/// never a concrete release, so the unit changes when the config changes
/// and stays byte-identical across routine deploys.
pub fn render(config: &Config) -> String {
    let root = config.root();
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
    for extra in &config.service.unit_extra {
        let _ = writeln!(unit, "{extra}");
    }
    let _ = writeln!(unit, "Restart=on-failure");
    let _ = writeln!(unit, "RestartSec=2");
    let _ = writeln!(unit);
    let _ = writeln!(unit, "[Install]");
    let _ = writeln!(unit, "WantedBy=multi-user.target");
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
        assert_eq!(render(&config), expected);
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
        let rendered = render(&config);
        assert!(rendered.contains("ExecStart=/srv/demo/current/demo\n"));
        assert!(!rendered.contains("User="));
        assert!(!rendered.contains("Environment="));
    }
}
