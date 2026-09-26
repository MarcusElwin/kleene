//! Persistent provider settings: what `kleene setup` writes and what
//! [`provider_from_env`](crate::provider_from_env) reads underneath the
//! environment.
//!
//! The file is `config.toml` in the Kleene config directory:
//! `$KLEENE_CONFIG_DIR`, else `$XDG_CONFIG_HOME/kleene`, else
//! `~/.config/kleene`. Environment variables always win over the file, so a
//! `KEY=... kleene run` one-off keeps working, and CI never needs the file.
//!
//! ```toml
//! router = "/home/me/.config/kleene/router.toml"   # optional; top level, before the tables
//!
//! [anthropic]
//! api_key = "sk-ant-..."
//!
//! [openai_compat]
//! api_key = "sk-..."
//! base_url = "https://api.openai.com/v1"
//! model = "gpt-5.4-mini"
//!
//! [typesafe]                      # Jev, the decision model behind jev_* and MODEL 'jev'
//! api_key = "ts-..."
//! model = "jev-latest"
//! ```

use crate::types::ProviderError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Directory override for the config file.
pub const CONFIG_DIR_ENV: &str = "KLEENE_CONFIG_DIR";
/// File name inside the config directory.
pub const CONFIG_FILE: &str = "config.toml";

/// Anthropic credentials.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnthropicSettings {
    /// `ANTHROPIC_API_KEY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// `ANTHROPIC_AUTH_TOKEN`, an OAuth token used instead of a key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    /// `ANTHROPIC_BASE_URL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl AnthropicSettings {
    /// Whether a credential is present.
    pub fn is_configured(&self) -> bool {
        non_empty(&self.api_key) || non_empty(&self.auth_token)
    }
}

/// OpenAI or OpenAI-compatible endpoint settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiCompatSettings {
    /// `OPENAI_API_KEY`; a local server may have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// `OPENAI_BASE_URL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// `OPENAI_MODEL`: the model every alias resolves to without a router.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl OpenAiCompatSettings {
    /// Whether the endpoint is usable: a key (OpenAI itself) or a URL (a
    /// local server or gateway).
    pub fn is_configured(&self) -> bool {
        non_empty(&self.api_key) || non_empty(&self.base_url)
    }
}

/// TypeSafe (Jev) decision-model settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeSafeSettings {
    /// `TYPESAFE_API_KEY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// `TYPESAFE_BASE_URL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// `TYPESAFE_DEFAULT_MODEL`: the model a decision uses when none is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl TypeSafeSettings {
    /// Whether a key is present.
    pub fn is_configured(&self) -> bool {
        non_empty(&self.api_key)
    }
}

/// Everything needed to build the provider stack.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// Anthropic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<AnthropicSettings>,
    /// OpenAI or compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_compat: Option<OpenAiCompatSettings>,
    /// TypeSafe's Jev, the decision provider. Optional: without it the
    /// `jev_*` functions fail with a clear message and nothing else changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typesafe: Option<TypeSafeSettings>,
    /// Path of a router TOML (`KLEENE_ROUTER_TOML`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router: Option<PathBuf>,
}

fn non_empty(v: &Option<String>) -> bool {
    v.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

impl ProviderSettings {
    /// The config directory: `$KLEENE_CONFIG_DIR`, `$XDG_CONFIG_HOME/kleene`
    /// or `~/.config/kleene`.
    pub fn dir() -> PathBuf {
        if let Some(d) = env(CONFIG_DIR_ENV) {
            return PathBuf::from(d);
        }
        if let Some(x) = env("XDG_CONFIG_HOME") {
            return PathBuf::from(x).join("kleene");
        }
        let home = env("HOME")
            .or_else(|| env("USERPROFILE"))
            .unwrap_or_else(|| ".".into());
        PathBuf::from(home).join(".config").join("kleene")
    }

    /// The config file path.
    pub fn path() -> PathBuf {
        Self::dir().join(CONFIG_FILE)
    }

    /// Settings present in the environment.
    pub fn from_env() -> Self {
        let anthropic = AnthropicSettings {
            api_key: env("ANTHROPIC_API_KEY"),
            auth_token: env("ANTHROPIC_AUTH_TOKEN"),
            base_url: env("ANTHROPIC_BASE_URL"),
        };
        let openai_compat = OpenAiCompatSettings {
            api_key: env("OPENAI_API_KEY"),
            base_url: env("OPENAI_BASE_URL"),
            model: env("OPENAI_MODEL"),
        };
        let typesafe = TypeSafeSettings {
            api_key: env(crate::adapters::typesafe::API_KEY_ENV),
            base_url: env(crate::adapters::typesafe::BASE_URL_ENV),
            model: env(crate::adapters::typesafe::DEFAULT_MODEL_ENV),
        };
        Self {
            anthropic: (anthropic != AnthropicSettings::default()).then_some(anthropic),
            openai_compat: (openai_compat != OpenAiCompatSettings::default())
                .then_some(openai_compat),
            typesafe: (typesafe != TypeSafeSettings::default()).then_some(typesafe),
            router: env(crate::env::ROUTER_TOML_ENV).map(PathBuf::from),
        }
    }

    /// Read the config file. `Ok(None)` when there is none.
    pub fn load() -> Result<Option<Self>, ProviderError> {
        Self::load_from(&Self::path())
    }

    /// Read a specific file. `Ok(None)` when it does not exist.
    pub fn load_from(path: &Path) -> Result<Option<Self>, ProviderError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(ProviderError::Other(format!(
                    "cannot read {}: {e}",
                    path.display()
                )))
            }
        };
        Self::parse(&text)
            .map(Some)
            .map_err(|e| ProviderError::Other(format!("{}: {e}", path.display())))
    }

    /// Parse the TOML form.
    pub fn parse(text: &str) -> Result<Self, ProviderError> {
        toml::from_str(text).map_err(|e| ProviderError::Other(e.to_string()))
    }

    /// Serialise to TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Write to the config file, creating the directory; the file is
    /// owner-readable only on Unix. Returns the path written.
    pub fn save(&self) -> Result<PathBuf, ProviderError> {
        let path = Self::path();
        self.save_to(&path)?;
        Ok(path)
    }

    /// Write to a specific path.
    pub fn save_to(&self, path: &Path) -> Result<(), ProviderError> {
        let io = |e: std::io::Error| ProviderError::Other(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        std::fs::write(path, self.to_toml()).map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
        }
        Ok(())
    }

    /// The file's settings with the environment layered on top: every field
    /// the environment sets wins, field by field.
    pub fn effective() -> Result<Self, ProviderError> {
        Ok(Self::load()?.unwrap_or_default().overlaid(Self::from_env()))
    }

    /// `other` on top of `self`, field by field.
    pub fn overlaid(mut self, other: Self) -> Self {
        if let Some(o) = other.anthropic {
            let mut a = self.anthropic.take().unwrap_or_default();
            a.api_key = o.api_key.or(a.api_key);
            a.auth_token = o.auth_token.or(a.auth_token);
            a.base_url = o.base_url.or(a.base_url);
            self.anthropic = Some(a);
        }
        if let Some(o) = other.openai_compat {
            let mut c = self.openai_compat.take().unwrap_or_default();
            c.api_key = o.api_key.or(c.api_key);
            c.base_url = o.base_url.or(c.base_url);
            c.model = o.model.or(c.model);
            self.openai_compat = Some(c);
        }
        if let Some(o) = other.typesafe {
            let mut t = self.typesafe.take().unwrap_or_default();
            t.api_key = o.api_key.or(t.api_key);
            t.base_url = o.base_url.or(t.base_url);
            t.model = o.model.or(t.model);
            self.typesafe = Some(t);
        }
        self.router = other.router.or(self.router);
        self
    }

    /// Whether at least one text provider can be built. The decision
    /// provider does not count: it answers questions, it cannot run a
    /// session.
    pub fn is_configured(&self) -> bool {
        self.configured().next().is_some()
    }

    /// Whether the decision provider (TypeSafe's Jev) can be built.
    pub fn decisions_configured(&self) -> bool {
        self.typesafe
            .as_ref()
            .is_some_and(TypeSafeSettings::is_configured)
    }

    /// Names of the providers that can be built, in registration order.
    pub fn configured(&self) -> impl Iterator<Item = &'static str> + '_ {
        let a = self
            .anthropic
            .as_ref()
            .is_some_and(AnthropicSettings::is_configured)
            .then_some("anthropic");
        let o = self
            .openai_compat
            .as_ref()
            .is_some_and(OpenAiCompatSettings::is_configured)
            .then_some("openai_compat");
        a.into_iter().chain(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let s = ProviderSettings {
            anthropic: Some(AnthropicSettings {
                api_key: Some("sk-ant-x".into()),
                ..Default::default()
            }),
            openai_compat: Some(OpenAiCompatSettings {
                api_key: None,
                base_url: Some("http://localhost:11434/v1".into()),
                model: Some("llama".into()),
            }),
            typesafe: Some(TypeSafeSettings {
                api_key: Some("ts-x".into()),
                base_url: None,
                model: None,
            }),
            router: None,
        };
        let text = s.to_toml();
        assert!(text.contains("[anthropic]"), "{text}");
        assert!(text.contains("[typesafe]"), "{text}");
        assert!(s.decisions_configured());
        assert!(
            !text.contains("auth_token"),
            "unset fields are omitted: {text}"
        );
        assert_eq!(ProviderSettings::parse(&text).unwrap(), s);
        assert_eq!(
            s.configured().collect::<Vec<_>>(),
            vec!["anthropic", "openai_compat"]
        );
    }

    #[test]
    fn overlay_is_field_by_field() {
        let file = ProviderSettings::parse(
            "[anthropic]\napi_key = 'file-key'\nbase_url = 'https://proxy'\n[openai_compat]\nmodel = 'm'\n",
        )
        .unwrap();
        let env = ProviderSettings {
            anthropic: Some(AnthropicSettings {
                api_key: Some("env-key".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let e = file.overlaid(env);
        let a = e.anthropic.unwrap();
        assert_eq!(a.api_key.as_deref(), Some("env-key"));
        assert_eq!(
            a.base_url.as_deref(),
            Some("https://proxy"),
            "kept from the file"
        );
        assert_eq!(e.openai_compat.unwrap().model.as_deref(), Some("m"));
    }

    #[test]
    fn empty_and_unusable_settings_are_not_configured() {
        assert!(!ProviderSettings::default().is_configured());
        let s = ProviderSettings::parse("[openai_compat]\nmodel = 'x'\n").unwrap();
        assert!(!s.is_configured(), "a model alone is not an endpoint");
        let s = ProviderSettings::parse("[openai_compat]\nbase_url = 'http://localhost:1/v1'\n")
            .unwrap();
        assert!(s.is_configured(), "a URL without a key is a local server");
        let s = ProviderSettings::parse("[typesafe]\napi_key = 'ts'\n").unwrap();
        assert!(
            !s.is_configured(),
            "a decision provider alone cannot run a session"
        );
        assert!(s.decisions_configured());
        let s = ProviderSettings::parse("[typesafe]\nmodel = 'jev-latest'\n").unwrap();
        assert!(!s.decisions_configured(), "a model alone is not a key");
    }

    #[test]
    fn save_and_load_a_file() {
        let dir = std::env::temp_dir().join(format!("kleene-config-{}", std::process::id()));
        let path = dir.join("nested").join(CONFIG_FILE);
        let s = ProviderSettings {
            anthropic: Some(AnthropicSettings {
                auth_token: Some("tok".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        s.save_to(&path).unwrap();
        assert_eq!(ProviderSettings::load_from(&path).unwrap(), Some(s));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert_eq!(
            ProviderSettings::load_from(&dir.join("missing.toml")).unwrap(),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
