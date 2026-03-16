use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use url::Url;

/// Configuration for a single Docker host
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HostConfig {
    /// Docker host connection string (e.g., "local", "ssh://user@host")
    pub host: String,

    /// Optional Dozzle URL for this host
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dozzle: Option<String>,

    /// Optional filters for this host (e.g., ["status=running", "name=nginx"])
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<Vec<String>>,
    // Future fields can be added here as optional fields
    // #[serde(skip_serializing_if = "Option::is_none")]
    // pub custom_name: Option<String>,
}

/// Configuration that can be loaded from a YAML file
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Config {
    /// Docker host(s) to connect to
    #[serde(default)]
    pub hosts: Vec<HostConfig>,

    /// Named host groups that can be selected with --group
    ///
    /// Each selector can be:
    /// - an exact host alias (e.g. "example-node-1")
    /// - an exact host spec (e.g. "ssh://example-node-1")
    /// - a prefix pattern ending in '*' (e.g. "example-group-a-*")
    #[serde(default)]
    pub groups: BTreeMap<String, Vec<String>>,

    /// Icon style to use (unicode or nerd)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<String>,

    /// Show all containers (default shows only running containers)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,

    /// Default sort field (uptime, name, cpu, memory)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
}

impl Config {
    /// Find and load config file from the following locations (in priority order):
    /// 1. ./config.yaml or ./config.yml
    /// 2. ./.dtop.yaml or ./.dtop.yml
    /// 3. ~/.config/dtop/config.yaml or ~/.config/dtop/config.yml
    /// 4. ~/.dtop.yaml or ~/.dtop.yml
    ///
    /// Returns (Config, Option<PathBuf>) where the PathBuf is Some if a config file was found
    pub fn load_with_path() -> Result<(Self, Option<PathBuf>), Box<dyn std::error::Error>> {
        let config_paths = Self::get_config_paths();

        for path in config_paths {
            if path.exists() {
                let contents = std::fs::read_to_string(&path)?;
                let config: Config = serde_yaml::from_str(&contents)?;
                return Ok((config, Some(path)));
            }
        }

        Ok((Config::default(), None))
    }

    /// Get list of potential config file paths in priority order
    fn get_config_paths() -> Vec<PathBuf> {
        // 1. Relative paths (current directory)
        let mut paths = vec![
            PathBuf::from("config.yaml"),
            PathBuf::from("config.yml"),
            PathBuf::from(".dtop.yaml"),
            PathBuf::from(".dtop.yml"),
        ];

        // 2. ~/.config/dtop/config.{yaml,yml}
        if let Some(home) = dirs::home_dir() {
            let config_dir = home.join(".config").join("dtop");
            paths.push(config_dir.join("config.yaml"));
            paths.push(config_dir.join("config.yml"));

            // 3. ~/.dtop.{yaml,yml}
            paths.push(home.join(".dtop.yaml"));
            paths.push(home.join(".dtop.yml"));
        }

        paths
    }

    /// Resolve named groups to a concrete, de-duplicated list of hosts.
    ///
    /// If a group is not explicitly defined in `groups`, dtop falls back to
    /// implicit matching on host aliases using:
    /// - the exact group name
    /// - the group name as a prefix pattern with `-*`
    ///
    /// This allows `--group example-group-a` or `--group example-group-b` to
    /// work naturally with host aliases such as
    /// `ssh://example-group-a-node-1` and `ssh://example-group-b-node-1`.
    pub fn resolve_groups(&self, group_names: &[String]) -> Result<Vec<HostConfig>, String> {
        let mut resolved = Vec::new();
        let mut seen_hosts = HashSet::new();
        let mut missing_groups = Vec::new();

        for group_name in group_names {
            let selectors = self
                .groups
                .get(group_name)
                .cloned()
                .unwrap_or_else(|| vec![group_name.clone(), format!("{group_name}-*")]);

            let matches: Vec<HostConfig> = self
                .hosts
                .iter()
                .filter(|host_config| {
                    selectors
                        .iter()
                        .any(|selector| Self::host_matches_selector(host_config, selector))
                })
                .cloned()
                .collect();

            if matches.is_empty() {
                missing_groups.push(group_name.clone());
                continue;
            }

            for host_config in matches {
                if seen_hosts.insert(host_config.host.clone()) {
                    resolved.push(host_config);
                }
            }
        }

        if !missing_groups.is_empty() {
            return Err(format!(
                "No hosts matched group(s): {}",
                missing_groups.join(", ")
            ));
        }

        Ok(resolved)
    }

    fn host_matches_selector(host_config: &HostConfig, selector: &str) -> bool {
        let alias = Self::host_alias(&host_config.host);
        Self::selector_matches(&host_config.host, selector)
            || Self::selector_matches(&alias, selector)
    }

    fn selector_matches(value: &str, selector: &str) -> bool {
        if let Some(prefix) = selector.strip_suffix('*') {
            value.starts_with(prefix)
        } else {
            value == selector
        }
    }

    fn host_alias(host_spec: &str) -> String {
        if host_spec == "local" {
            return "local".to_string();
        }

        if let Ok(url) = Url::parse(host_spec) {
            if let Some(host) = url.host_str() {
                return host.to_string();
            }
        }

        host_spec.to_string()
    }

    fn with_selected_hosts(
        mut self,
        selected_hosts: Vec<HostConfig>,
        cli_filters: Vec<String>,
    ) -> Self {
        self.hosts = selected_hosts;

        if !cli_filters.is_empty() {
            for host_config in &mut self.hosts {
                host_config.filter = Some(cli_filters.clone());
            }
        }

        self
    }

    fn apply_cli_overrides(&mut self, cli_all: bool, cli_sort: Option<String>) {
        // CLI 'all' flag can only enable showing all containers, not disable it
        // This matches docker ps -a behavior: it's a simple boolean flag
        // If config has all: true, users must edit config or use 'a' key in UI to toggle
        if cli_all {
            self.all = Some(true);
        }
        // When cli_all is false, config value is preserved (config is not overridden)

        // CLI sort takes precedence over config sort
        if cli_sort.is_some() {
            self.sort = cli_sort;
        }
        // When cli_sort is None, config value is preserved
    }

    /// Merge config with a selected subset of config-defined hosts.
    pub fn merge_with_selected_hosts(
        mut self,
        selected_hosts: Vec<HostConfig>,
        cli_filters: Vec<String>,
        cli_all: bool,
        cli_sort: Option<String>,
    ) -> Self {
        self = self.with_selected_hosts(selected_hosts, cli_filters);
        self.apply_cli_overrides(cli_all, cli_sort);
        self
    }

    /// Merge config with command line arguments
    /// CLI args take precedence over config file values (with exceptions noted below)
    pub fn merge_with_cli_hosts(
        mut self,
        cli_hosts: Vec<String>,
        cli_default: bool,
        cli_filters: Vec<String>,
        cli_all: bool,
        cli_sort: Option<String>,
    ) -> Self {
        // Use CLI hosts if explicitly provided, OR if config file is empty
        if !cli_default || self.hosts.is_empty() {
            // Convert CLI strings to HostConfig structs
            let selected_hosts = cli_hosts
                .into_iter()
                .map(|host| HostConfig {
                    host,
                    dozzle: None,
                    filter: if cli_filters.is_empty() {
                        None
                    } else {
                        Some(cli_filters.clone())
                    },
                })
                .collect();
            self = self.with_selected_hosts(selected_hosts, vec![]);
        } else if !cli_filters.is_empty() {
            // Config file hosts are being used, but CLI filters override per-host filters
            for host_config in &mut self.hosts {
                host_config.filter = Some(cli_filters.clone());
            }
        }

        self.apply_cli_overrides(cli_all, cli_sort);

        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.hosts.is_empty());
    }

    #[test]
    fn test_merge_with_cli_hosts_uses_cli_when_provided() {
        let config = Config {
            hosts: vec![HostConfig {
                host: "ssh://user@example-host-1".to_string(),
                dozzle: None,
                filter: None,
            }],
            groups: BTreeMap::new(),
            icons: None,
            all: None,
            sort: None,
        };

        let merged = config.merge_with_cli_hosts(
            vec!["ssh://user@example-host-2".to_string()],
            false,
            vec![],
            false,
            None,
        );
        assert_eq!(merged.hosts.len(), 1);
        assert_eq!(merged.hosts[0].host, "ssh://user@example-host-2");
    }

    #[test]
    fn test_merge_with_cli_hosts_uses_config_when_cli_is_default() {
        let config = Config {
            hosts: vec![HostConfig {
                host: "ssh://user@example-host-1".to_string(),
                dozzle: Some("https://dozzle.example.com".to_string()),
                filter: None,
            }],
            groups: BTreeMap::new(),
            icons: None,
            all: None,
            sort: None,
        };

        let merged =
            config.merge_with_cli_hosts(vec!["local".to_string()], true, vec![], false, None);
        assert_eq!(merged.hosts.len(), 1);
        assert_eq!(merged.hosts[0].host, "ssh://user@example-host-1");
        // Config file's dozzle URL is preserved
        assert_eq!(
            merged.hosts[0].dozzle.as_deref(),
            Some("https://dozzle.example.com")
        );
    }

    #[test]
    fn test_merge_with_cli_hosts_defaults_to_local() {
        let config = Config {
            hosts: vec![],
            groups: BTreeMap::new(),
            icons: None,
            all: None,
            sort: None,
        };

        let merged =
            config.merge_with_cli_hosts(vec!["local".to_string()], true, vec![], false, None);
        assert_eq!(merged.hosts.len(), 1);
        assert_eq!(merged.hosts[0].host, "local");
    }

    #[test]
    fn test_yaml_deserialization() {
        let yaml = r#"
hosts:
  - host: local
  - host: ssh://user@example-host-1
  - host: ssh://user@example-host-2:2222
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.hosts.len(), 3);
        assert_eq!(config.hosts[0].host, "local");
        assert_eq!(config.hosts[1].host, "ssh://user@example-host-1");
        assert_eq!(config.hosts[2].host, "ssh://user@example-host-2:2222");
        assert_eq!(config.hosts[0].dozzle, None);
    }

    #[test]
    fn test_yaml_deserialization_with_dozzle() {
        let yaml = r#"
hosts:
  - host: ssh://user@example-host-1
    dozzle: https://logs.example.com/
  - host: local
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.hosts.len(), 2);
        assert_eq!(config.hosts[0].host, "ssh://user@example-host-1");
        assert_eq!(config.hosts[1].host, "local");
        assert_eq!(
            config.hosts[0].dozzle.as_deref(),
            Some("https://logs.example.com/")
        );
        assert_eq!(config.hosts[1].dozzle, None);
    }

    #[test]
    fn test_host_config_without_dozzle() {
        let host = HostConfig {
            host: "local".to_string(),
            dozzle: None,
            filter: None,
        };
        assert_eq!(host.host, "local");
        assert_eq!(host.dozzle, None);
        assert_eq!(host.filter, None);
    }

    #[test]
    fn test_host_config_with_dozzle() {
        let host = HostConfig {
            host: "ssh://user@host".to_string(),
            dozzle: Some("https://dozzle.example.com".to_string()),
            filter: None,
        };
        assert_eq!(host.host, "ssh://user@host");
        assert_eq!(host.dozzle.as_deref(), Some("https://dozzle.example.com"));
    }

    #[test]
    fn test_merge_cli_filters_override_config() {
        let config = Config {
            hosts: vec![HostConfig {
                host: "local".to_string(),
                dozzle: None,
                filter: Some(vec!["status=running".to_string()]),
            }],
            groups: BTreeMap::new(),
            icons: None,
            all: None,
            sort: None,
        };

        let cli_filters = vec!["name=nginx".to_string()];
        let merged =
            config.merge_with_cli_hosts(vec!["local".to_string()], true, cli_filters, false, None);
        assert_eq!(merged.hosts.len(), 1);
        assert_eq!(
            merged.hosts[0].filter.as_ref().unwrap(),
            &vec!["name=nginx".to_string()]
        );
    }

    #[test]
    fn test_config_filters_preserved_when_no_cli_filters() {
        let config = Config {
            hosts: vec![HostConfig {
                host: "local".to_string(),
                dozzle: None,
                filter: Some(vec!["status=running".to_string()]),
            }],
            groups: BTreeMap::new(),
            icons: None,
            all: None,
            sort: None,
        };

        let merged =
            config.merge_with_cli_hosts(vec!["local".to_string()], true, vec![], false, None);
        assert_eq!(merged.hosts.len(), 1);
        assert_eq!(
            merged.hosts[0].filter.as_ref().unwrap(),
            &vec!["status=running".to_string()]
        );
    }

    #[test]
    fn test_cli_all_flag_overrides_config() {
        let config = Config {
            hosts: vec![HostConfig {
                host: "local".to_string(),
                dozzle: None,
                filter: None,
            }],
            groups: BTreeMap::new(),
            icons: None,
            all: Some(false), // Config says false
            sort: None,
        };

        let merged =
            config.merge_with_cli_hosts(vec!["local".to_string()], true, vec![], true, None); // CLI says true
        assert_eq!(merged.all, Some(true)); // CLI should win
    }

    #[test]
    fn test_config_all_preserved_when_cli_not_set() {
        let config = Config {
            hosts: vec![HostConfig {
                host: "local".to_string(),
                dozzle: None,
                filter: None,
            }],
            groups: BTreeMap::new(),
            icons: None,
            all: Some(true), // Config says true
            sort: None,
        };

        let merged =
            config.merge_with_cli_hosts(vec!["local".to_string()], true, vec![], false, None); // CLI not set
        assert_eq!(merged.all, Some(true)); // Config value should be preserved when CLI is false
    }

    #[test]
    fn test_all_defaults_to_none() {
        let config = Config {
            hosts: vec![HostConfig {
                host: "local".to_string(),
                dozzle: None,
                filter: None,
            }],
            groups: BTreeMap::new(),
            icons: None,
            all: None, // No config value
            sort: None,
        };

        let merged =
            config.merge_with_cli_hosts(vec!["local".to_string()], true, vec![], false, None); // CLI false
        assert_eq!(merged.all, None); // Should remain None (will default to false in main.rs)
    }

    #[test]
    fn test_cli_sort_overrides_config() {
        let config = Config {
            hosts: vec![HostConfig {
                host: "local".to_string(),
                dozzle: None,
                filter: None,
            }],
            groups: BTreeMap::new(),
            icons: None,
            all: None,
            sort: Some("name".to_string()), // Config says name
        };

        let merged = config.merge_with_cli_hosts(
            vec!["local".to_string()],
            true,
            vec![],
            false,
            Some("cpu".to_string()), // CLI says cpu
        );
        assert_eq!(merged.sort, Some("cpu".to_string())); // CLI should win
    }

    #[test]
    fn test_config_sort_preserved_when_cli_not_set() {
        let config = Config {
            hosts: vec![HostConfig {
                host: "local".to_string(),
                dozzle: None,
                filter: None,
            }],
            groups: BTreeMap::new(),
            icons: None,
            all: None,
            sort: Some("memory".to_string()), // Config says memory
        };

        let merged =
            config.merge_with_cli_hosts(vec!["local".to_string()], true, vec![], false, None); // CLI not set
        assert_eq!(merged.sort, Some("memory".to_string())); // Config value should be preserved
    }

    #[test]
    fn test_yaml_deserialization_with_sort() {
        let yaml = r#"
hosts:
  - host: local
sort: cpu
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.hosts.len(), 1);
        assert_eq!(config.sort, Some("cpu".to_string()));
    }

    #[test]
    fn test_yaml_deserialization_with_groups() {
        let yaml = r#"
hosts:
  - host: ssh://example-group-a-node-1
groups:
  example-group-a:
    - example-group-a-*
  example-group-a-admin:
    - example-group-a-admin-*
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.groups["example-group-a"],
            vec!["example-group-a-*".to_string()]
        );
        assert_eq!(
            config.groups["example-group-a-admin"],
            vec!["example-group-a-admin-*".to_string()]
        );
    }

    #[test]
    fn test_resolve_groups_uses_implicit_prefix_matching() {
        let config = Config {
            hosts: vec![
                HostConfig {
                    host: "ssh://example-group-a-node-1".to_string(),
                    dozzle: None,
                    filter: None,
                },
                HostConfig {
                    host: "ssh://example-group-a-node-2".to_string(),
                    dozzle: None,
                    filter: None,
                },
                HostConfig {
                    host: "ssh://example-group-b-node-1".to_string(),
                    dozzle: None,
                    filter: None,
                },
            ],
            groups: BTreeMap::new(),
            icons: None,
            all: None,
            sort: None,
        };

        let resolved = config
            .resolve_groups(&["example-group-a".to_string()])
            .unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].host, "ssh://example-group-a-node-1");
        assert_eq!(resolved[1].host, "ssh://example-group-a-node-2");
    }

    #[test]
    fn test_resolve_groups_uses_explicit_group_patterns() {
        let config = Config {
            hosts: vec![
                HostConfig {
                    host: "ssh://example-group-b-node-1".to_string(),
                    dozzle: None,
                    filter: None,
                },
                HostConfig {
                    host: "ssh://example-group-b-node-2".to_string(),
                    dozzle: None,
                    filter: None,
                },
                HostConfig {
                    host: "ssh://example-group-c-node-1".to_string(),
                    dozzle: None,
                    filter: None,
                },
            ],
            groups: BTreeMap::from([(
                "example-group-b".to_string(),
                vec!["example-group-b-*".to_string()],
            )]),
            icons: None,
            all: None,
            sort: None,
        };

        let resolved = config
            .resolve_groups(&["example-group-b".to_string()])
            .unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].host, "ssh://example-group-b-node-1");
        assert_eq!(resolved[1].host, "ssh://example-group-b-node-2");
    }
}
