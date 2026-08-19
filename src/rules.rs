use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rule {
    pub action: String,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<FeatureRule>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FeatureRule {
    #[serde(default)]
    pub is_demo_user: Option<bool>,
    #[serde(default)]
    pub has_custom_resolution: Option<bool>,
    #[serde(default)]
    pub has_quick_plays_support: Option<bool>,
    #[serde(default)]
    pub is_quick_play_singleplayer: Option<bool>,
    #[serde(default)]
    pub is_quick_play_multiplayer: Option<bool>,
    #[serde(default)]
    pub is_quick_play_realms: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct Features {
    pub demo: bool,
    pub custom_resolution: bool,
}

pub fn current_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

pub fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    }
}

pub fn rules_allow(rules: &[Rule], features: &Features) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allow = false;
    for rule in rules {
        if rule_matches(rule, features) {
            allow = rule.action == "allow";
        }
    }
    allow
}

fn rule_matches(rule: &Rule, features: &Features) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name {
            if name != current_os() {
                return false;
            }
        }
        if let Some(arch) = &os.arch {
            let ours = current_arch();
            let ok = arch == ours
                || (arch == "x86" && ours == "x86")
                || (arch == "x64" && ours == "x86_64");
            if !ok {
                return false;
            }
        }
        if os.version.is_some() {
            // Mojang uses this for Windows 10 heuristics; ignore the regex.
        }
    }
    if let Some(feat) = &rule.features {
        if !feature_ok(feat.is_demo_user, features.demo) {
            return false;
        }
        if !feature_ok(feat.has_custom_resolution, features.custom_resolution) {
            return false;
        }
        if feat.has_quick_plays_support == Some(true)
            || feat.is_quick_play_singleplayer == Some(true)
            || feat.is_quick_play_multiplayer == Some(true)
            || feat.is_quick_play_realms == Some(true)
        {
            return false;
        }
    }
    true
}

fn feature_ok(expected: Option<bool>, actual: bool) -> bool {
    match expected {
        Some(value) => value == actual,
        None => true,
    }
}

pub fn parse_rules(value: &Value) -> Vec<Rule> {
    serde_json::from_value(value.get("rules").cloned().unwrap_or(Value::Null)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_rules_allow() {
        assert!(rules_allow(&[], &Features::default()));
    }

    #[test]
    fn linux_disallow_windows() {
        let rules = vec![
            Rule {
                action: "allow".into(),
                ..Default::default()
            },
            Rule {
                action: "disallow".into(),
                os: Some(OsRule {
                    name: Some("windows".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];
        if current_os() == "linux" {
            assert!(rules_allow(&rules, &Features::default()));
        }
    }
}
