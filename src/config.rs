use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub telegram: TelegramConfig,
    pub llm: LlmConfig,
    pub timeouts: TimeoutsConfig,
    pub cooldown: CooldownConfig,
    #[serde(default)]
    pub i18n: I18nConfig,
    #[serde(default)]
    pub groups: Vec<GroupConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramConfig {
    pub bot_token: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmConfig {
    #[serde(default = "default_openai_base_url")]
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default = "default_llm_request_timeout")]
    pub request_timeout_secs: u64,
    /// How many recently-asked questions (per group) to feed back into the
    /// question-generation prompt as a "do not repeat" list. Set to 0 to
    /// disable.
    #[serde(default = "default_recent_question_window")]
    pub recent_question_window: u32,
}

fn default_openai_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

fn default_llm_request_timeout() -> u64 {
    30
}

fn default_recent_question_window() -> u32 {
    20
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TimeoutsConfig {
    pub button_press_secs: u64,
    pub answer_submission_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CooldownConfig {
    pub retry_after_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct I18nConfig {
    #[serde(default = "default_locale")]
    pub default_locale: String,
}

impl Default for I18nConfig {
    fn default() -> Self {
        Self {
            default_locale: default_locale(),
        }
    }
}

fn default_locale() -> String {
    "en".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroupConfig {
    pub id: i64,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub question_prompt: String,
}

fn default_true() -> bool {
    true
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|source| Error::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Config =
            serde_yaml_ng::from_slice(&bytes).map_err(|source| Error::ConfigParse {
                path: path.to_path_buf(),
                source,
            })?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.telegram.bot_token.trim().is_empty() {
            return Err(Error::ConfigInvalid(
                "telegram.bot_token must not be empty".into(),
            ));
        }
        if self.llm.api_key.trim().is_empty() {
            return Err(Error::ConfigInvalid("llm.api_key must not be empty".into()));
        }
        if self.llm.model.trim().is_empty() {
            return Err(Error::ConfigInvalid("llm.model must not be empty".into()));
        }
        if self.timeouts.button_press_secs == 0 {
            return Err(Error::ConfigInvalid(
                "timeouts.button_press_secs must be > 0".into(),
            ));
        }
        if self.timeouts.answer_submission_secs == 0 {
            return Err(Error::ConfigInvalid(
                "timeouts.answer_submission_secs must be > 0".into(),
            ));
        }
        let mut seen: HashMap<i64, ()> = HashMap::new();
        for group in &self.groups {
            if group.question_prompt.trim().is_empty() {
                return Err(Error::ConfigInvalid(format!(
                    "group {} has empty question_prompt",
                    group.id
                )));
            }
            if seen.insert(group.id, ()).is_some() {
                return Err(Error::ConfigInvalid(format!(
                    "group {} is listed more than once",
                    group.id
                )));
            }
        }
        Ok(())
    }

    pub fn group(&self, id: i64) -> Option<&GroupConfig> {
        self.groups.iter().find(|g| g.id == id)
    }
}

pub fn default_config_path() -> PathBuf {
    PathBuf::from("configs/bouncer.yaml")
}

pub fn default_database_path() -> PathBuf {
    PathBuf::from("bouncer.db")
}
