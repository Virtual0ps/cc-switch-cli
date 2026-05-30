use std::path::PathBuf;

use crate::config::{
    atomic_write, delete_file, home_dir, read_json_file, sanitize_provider_name, write_json_file,
    write_text_file,
};
use crate::error::AppError;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use toml_edit::DocumentMut;

pub const CC_SWITCH_CODEX_MODEL_PROVIDER_ID: &str = "custom";

/// Reserved built-in provider IDs from OpenAI Codex's config/model-provider
/// catalog. Keep in sync with Codex `RESERVED_MODEL_PROVIDER_IDS` and legacy
/// removed provider aliases.
const CODEX_RESERVED_MODEL_PROVIDER_IDS: &[&str] = &[
    "amazon-bedrock",
    "openai",
    "ollama",
    "lmstudio",
    "oss",
    "ollama-chat",
];

/// 获取 Codex 配置目录路径
pub fn get_codex_config_dir() -> PathBuf {
    if let Some(custom) = crate::settings::get_codex_override_dir() {
        return custom;
    }

    if let Some(dir) = std::env::var_os("CODEX_HOME") {
        let dir = PathBuf::from(dir);
        if !dir.as_os_str().is_empty() && !dir.to_string_lossy().trim().is_empty() && dir.is_dir() {
            return dir;
        }
    }

    home_dir().expect("无法获取用户主目录").join(".codex")
}

/// 获取 Codex auth.json 路径
pub fn get_codex_auth_path() -> PathBuf {
    get_codex_config_dir().join("auth.json")
}

/// 获取 Codex config.toml 路径
pub fn get_codex_config_path() -> PathBuf {
    get_codex_config_dir().join("config.toml")
}

/// 获取 Codex 供应商配置文件路径
pub fn get_codex_provider_paths(
    provider_id: &str,
    provider_name: Option<&str>,
) -> (PathBuf, PathBuf) {
    let base_name = provider_name
        .map(sanitize_provider_name)
        .unwrap_or_else(|| sanitize_provider_name(provider_id));

    let auth_path = get_codex_config_dir().join(format!("auth-{base_name}.json"));
    let config_path = get_codex_config_dir().join(format!("config-{base_name}.toml"));

    (auth_path, config_path)
}

/// 删除 Codex 供应商配置文件
pub fn delete_codex_provider_config(
    provider_id: &str,
    provider_name: &str,
) -> Result<(), AppError> {
    let (auth_path, config_path) = get_codex_provider_paths(provider_id, Some(provider_name));

    delete_file(&auth_path).ok();
    delete_file(&config_path).ok();

    Ok(())
}

/// 原子写 Codex 的 `auth.json` 与 `config.toml`，在第二步失败时回滚第一步
pub fn write_codex_live_atomic(
    auth: &Value,
    config_text_opt: Option<&str>,
) -> Result<(), AppError> {
    write_codex_live_atomic_optional_auth(Some(auth), config_text_opt)
}

pub fn write_codex_live_atomic_optional_auth(
    auth: Option<&Value>,
    config_text_opt: Option<&str>,
) -> Result<(), AppError> {
    let auth_path = get_codex_auth_path();
    let config_path = get_codex_config_path();

    if let Some(parent) = auth_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    // 读取旧内容用于回滚
    let old_auth = if auth_path.exists() {
        Some(fs::read(&auth_path).map_err(|e| AppError::io(&auth_path, e))?)
    } else {
        None
    };
    let _old_config = if config_path.exists() {
        Some(fs::read(&config_path).map_err(|e| AppError::io(&config_path, e))?)
    } else {
        None
    };

    // 准备写入内容
    let cfg_text = match config_text_opt {
        Some(s) => s.to_string(),
        None => String::new(),
    };
    if !cfg_text.trim().is_empty() {
        toml::from_str::<toml::Table>(&cfg_text).map_err(|e| AppError::toml(&config_path, e))?;
    }

    // 第一步：写 auth.json
    if let Some(auth) = auth {
        write_json_file(&auth_path, auth)?;
    } else {
        delete_file(&auth_path)?;
    }

    // 第二步：写 config.toml（失败则回滚 auth.json）
    if let Err(e) = write_text_file(&config_path, &cfg_text) {
        // 回滚 auth.json
        if let Some(bytes) = old_auth {
            let _ = atomic_write(&auth_path, &bytes);
        } else {
            let _ = delete_file(&auth_path);
        }
        return Err(e);
    }

    Ok(())
}

/// 读取 `~/.codex/config.toml`，若不存在返回空字符串
pub fn read_codex_config_text() -> Result<String, AppError> {
    let path = get_codex_config_path();
    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))
    } else {
        Ok(String::new())
    }
}

/// 对非空的 TOML 文本进行语法校验
pub fn validate_config_toml(text: &str) -> Result<(), AppError> {
    if text.trim().is_empty() {
        return Ok(());
    }
    toml::from_str::<toml::Table>(text)
        .map(|_| ())
        .map_err(|e| AppError::toml(Path::new("config.toml"), e))
}

/// Remove provider-specific Codex TOML keys and keep only shared/global settings.
///
/// This matches upstream "OpenAI Official" snapshot semantics where the official
/// provider does not persist a provider-local `base_url` / `model_provider`
/// section, but may still carry root-level shared settings.
pub fn strip_codex_provider_config_text(config_toml: &str) -> Result<String, AppError> {
    let config_toml = config_toml.trim();
    if config_toml.is_empty() {
        return Ok(String::new());
    }

    let mut doc = config_toml
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| AppError::Config(format!("TOML parse error: {e}")))?;
    let root = doc.as_table_mut();
    root.remove("model");
    root.remove("model_provider");
    root.remove("base_url");
    root.remove("model_providers");

    let mut cleaned = String::new();
    let mut blank_run = 0usize;
    for line in doc.to_string().lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                cleaned.push('\n');
            }
            continue;
        }
        blank_run = 0;
        cleaned.push_str(line);
        cleaned.push('\n');
    }

    Ok(cleaned.trim().to_string())
}

/// 读取并校验 `~/.codex/config.toml`，返回文本（可能为空）
pub fn read_and_validate_codex_config_text() -> Result<String, AppError> {
    let s = read_codex_config_text()?;
    validate_config_toml(&s)?;
    Ok(s)
}

fn active_codex_model_provider_id(doc: &DocumentMut) -> Option<String> {
    doc.get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

pub(crate) fn is_custom_codex_model_provider_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && !CODEX_RESERVED_MODEL_PROVIDER_IDS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(id))
}

/// Write only Codex `config.toml` for provider switching.
///
/// Codex login state lives in `auth.json`; provider routing, endpoint, model,
/// and provider-scoped bearer tokens live in `config.toml`. Provider switches
/// should not overwrite the user's ChatGPT login cache.
pub fn write_codex_live_config_atomic(config_text_opt: Option<&str>) -> Result<(), AppError> {
    let config_path = get_codex_config_path();
    let cfg_text = match config_text_opt {
        Some(config_text) => config_text.to_string(),
        None => String::new(),
    };

    if !cfg_text.trim().is_empty() {
        toml::from_str::<toml::Table>(&cfg_text).map_err(|e| AppError::toml(&config_path, e))?;
    }

    write_text_file(&config_path, &cfg_text)
}

pub fn extract_codex_auth_api_key(auth: &Value) -> Option<String> {
    auth.get("OPENAI_API_KEY")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

pub fn extract_codex_api_key(auth: Option<&Value>, config_text: Option<&str>) -> Option<String> {
    auth.and_then(extract_codex_auth_api_key)
        .or_else(|| config_text.and_then(extract_codex_experimental_bearer_token))
}

pub fn codex_auth_has_login_material(auth: &Value) -> bool {
    let Some(obj) = auth.as_object() else {
        return false;
    };

    obj.iter().any(|(key, value)| {
        if key == "auth_mode" {
            return false;
        }

        if key == "OPENAI_API_KEY" {
            return value
                .as_str()
                .map(str::trim)
                .is_some_and(|token| !token.is_empty());
        }

        match value {
            Value::Null => false,
            Value::String(text) => !text.trim().is_empty(),
            Value::Array(items) => !items.is_empty(),
            Value::Object(map) => !map.is_empty(),
            _ => true,
        }
    })
}

pub fn codex_auth_has_oauth_login_material(auth: &Value) -> bool {
    let Some(obj) = auth.as_object() else {
        return false;
    };

    obj.iter().any(|(key, value)| {
        if key == "auth_mode" || key == "OPENAI_API_KEY" {
            return false;
        }

        match value {
            Value::Null => false,
            Value::String(text) => !text.trim().is_empty(),
            Value::Array(items) => !items.is_empty(),
            Value::Object(map) => !map.is_empty(),
            _ => true,
        }
    })
}

pub fn should_restore_codex_provider_token_for_backfill(
    category: Option<&str>,
    template_settings: &Value,
) -> bool {
    if category == Some("official") {
        return false;
    }

    let Some(auth) = template_settings.get("auth") else {
        return true;
    };

    let has_provider_api_key = extract_codex_auth_api_key(auth).is_some();
    let has_oauth_login = codex_auth_has_oauth_login_material(auth);
    !has_oauth_login || has_provider_api_key
}

/// Extract a provider-scoped `experimental_bearer_token` from Codex `config.toml`.
///
/// Third-party providers may store the API key inside
/// `[model_providers.<id>].experimental_bearer_token` while keeping the
/// user's ChatGPT login cache intact in `auth.json`. Falls back to the
/// top-level `experimental_bearer_token` when no active model provider is set.
pub fn extract_codex_experimental_bearer_token(config_text: &str) -> Option<String> {
    if !config_text.contains("experimental_bearer_token") {
        return None;
    }
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let provider_id = active_codex_model_provider_id(&doc);

    let top_level_token = || {
        doc.get("experimental_bearer_token")
            .and_then(|item| item.as_str())
    };
    let token = match provider_id.as_deref() {
        Some(id) if is_custom_codex_model_provider_id(id) => doc
            .get("model_providers")
            .and_then(|item| item.as_table())
            .and_then(|table| table.get(id))
            .and_then(|item| item.as_table())
            .and_then(|table| table.get("experimental_bearer_token"))
            .and_then(|item| item.as_str())
            .or_else(top_level_token),
        Some(_) => top_level_token(),
        None => top_level_token(),
    };

    token
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn set_codex_experimental_bearer_token(config_text: &str, token: &str) -> Result<String, AppError> {
    if config_text.trim().is_empty() {
        return Err(AppError::localized(
            "provider.codex.config.missing",
            "Codex 第三方供应商缺少 config.toml 配置，无法写入 bearer token",
            "Codex third-party provider is missing config.toml, cannot write bearer token",
        ));
    }

    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;

    let Some(provider_id) = active_codex_model_provider_id(&doc) else {
        doc["experimental_bearer_token"] = toml_edit::value(token);
        return Ok(doc.to_string());
    };

    if !is_custom_codex_model_provider_id(&provider_id) {
        // Reserved Codex provider IDs are owned by the CLI. Keep third-party
        // bearer tokens at the top level so we do not shadow built-in tables.
        doc["experimental_bearer_token"] = toml_edit::value(token);
        return Ok(doc.to_string());
    }

    if let Some(model_providers) = doc
        .get_mut("model_providers")
        .and_then(|item| item.as_table_mut())
    {
        if let Some(provider_table) = model_providers
            .get_mut(provider_id.as_str())
            .and_then(|item| item.as_table_mut())
        {
            provider_table["experimental_bearer_token"] = toml_edit::value(token);
            return Ok(doc.to_string());
        }
    }

    doc["experimental_bearer_token"] = toml_edit::value(token);
    Ok(doc.to_string())
}

fn remove_codex_experimental_bearer_token(config_text: &str) -> Result<String, AppError> {
    if config_text.trim().is_empty() || !config_text.contains("experimental_bearer_token") {
        return Ok(config_text.to_string());
    }

    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;

    if let Some(provider_id) = active_codex_model_provider_id(&doc) {
        if let Some(provider_table) = doc
            .get_mut("model_providers")
            .and_then(|item| item.as_table_mut())
            .and_then(|table| table.get_mut(provider_id.as_str()))
            .and_then(|item| item.as_table_mut())
        {
            provider_table.remove("experimental_bearer_token");
        }
    }

    doc.as_table_mut().remove("experimental_bearer_token");
    Ok(doc.to_string())
}

/// Read the current Codex live settings as a `{ auth, config }` object.
///
/// Missing `auth.json` collapses to `{}` so a config-only third-party install
/// is still importable; both files empty is treated as "no live install".
pub fn read_codex_live_settings() -> Result<Value, AppError> {
    let auth_path = get_codex_auth_path();
    let auth_present = auth_path.exists();
    let auth: Value = if auth_present {
        read_json_file(&auth_path)?
    } else {
        json!({})
    };
    let cfg_text = read_and_validate_codex_config_text()?;
    if !auth_present && cfg_text.trim().is_empty() {
        return Err(AppError::localized(
            "codex.live.missing",
            "Codex 配置文件不存在",
            "Codex configuration is missing",
        ));
    }
    Ok(json!({ "auth": auth, "config": cfg_text }))
}

/// Route a Codex live write between full auth+config or config-only.
///
/// Official providers with usable login material own `auth.json`; everyone
/// else only touches `config.toml` so the user's ChatGPT login cache survives
/// third-party switches.
pub fn write_codex_live_for_provider(
    category: Option<&str>,
    auth: &Value,
    config_text: Option<&str>,
) -> Result<(), AppError> {
    if category == Some("official") && codex_auth_has_login_material(auth) {
        write_codex_live_atomic(auth, config_text)
    } else {
        let live_config = prepare_codex_provider_live_config(auth, config_text.unwrap_or(""))?;
        write_codex_live_config_atomic(Some(&live_config))
    }
}

/// Build the live Codex config for provider switching.
///
/// The stored provider keeps its API key in `auth.OPENAI_API_KEY`. Live Codex
/// requests can use a provider-scoped `experimental_bearer_token`, so switching
/// providers only needs to update `config.toml`; `auth.json` stays as the user's
/// long-lived ChatGPT login cache.
pub fn prepare_codex_provider_live_config(
    auth: &Value,
    config_text: &str,
) -> Result<String, AppError> {
    let token = extract_codex_auth_api_key(auth)
        .or_else(|| extract_codex_experimental_bearer_token(config_text));

    Ok(match token {
        Some(token) => set_codex_experimental_bearer_token(config_text, &token)?,
        None => config_text.to_string(),
    })
}

/// During DB backfill, lift a live `experimental_bearer_token` back into
/// `auth.OPENAI_API_KEY` so the stored provider keeps its canonical shape
/// and generated live tokens don't leak into stored provider TOML.
///
/// Only intervenes when the live config actually carries a bearer token;
/// otherwise the function is a no-op so the caller's normal backfill path
/// remains authoritative.
pub fn restore_codex_provider_token_for_backfill(
    settings: &mut Value,
    template_settings: &Value,
) -> Result<(), AppError> {
    let Some(config_text) = settings
        .get("config")
        .and_then(|value| value.as_str())
        .map(str::to_string)
    else {
        return Ok(());
    };

    let Some(token) = extract_codex_experimental_bearer_token(&config_text) else {
        return Ok(());
    };

    let cleaned_config = remove_codex_experimental_bearer_token(&config_text)?;

    if let Some(obj) = settings.as_object_mut() {
        obj.insert("config".to_string(), Value::String(cleaned_config));

        let mut auth = template_settings
            .get("auth")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        if let Some(auth_obj) = auth.as_object_mut() {
            auth_obj.insert("OPENAI_API_KEY".to_string(), Value::String(token));
        }
        obj.insert("auth".to_string(), auth);
    }

    Ok(())
}

pub fn restore_codex_settings_for_backfill(
    settings: &mut Value,
    template_settings: &Value,
    restore_provider_token: bool,
) -> Result<(), AppError> {
    if restore_provider_token {
        restore_codex_provider_token_for_backfill(settings, template_settings)?;
    }
    Ok(())
}

/// Generate a clean TOML key from a raw string for use as `model_provider` and `[model_providers.<key>]`.
///
/// Lowercases ASCII alphanumerics, replaces everything else with `_`, trims leading/trailing `_`.
/// Falls back to `"custom"` if the result is empty.
pub fn clean_codex_provider_key(raw: &str) -> String {
    let mut key: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();

    while key.starts_with('_') {
        key.remove(0);
    }
    while key.ends_with('_') {
        key.pop();
    }

    if key.is_empty() {
        "custom".to_string()
    } else {
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{lock_test_home_and_settings, set_test_home_override};
    use std::env;
    use std::ffi::OsString;

    struct CodexHomeEnvGuard {
        original: Option<OsString>,
    }

    impl CodexHomeEnvGuard {
        fn new(value: Option<&str>) -> Self {
            let original = env::var_os("CODEX_HOME");
            match value {
                Some(value) => unsafe { env::set_var("CODEX_HOME", value) },
                None => unsafe { env::remove_var("CODEX_HOME") },
            }
            Self { original }
        }
    }

    impl Drop for CodexHomeEnvGuard {
        fn drop(&mut self) {
            match self.original.as_ref() {
                Some(value) => unsafe { env::set_var("CODEX_HOME", value) },
                None => unsafe { env::remove_var("CODEX_HOME") },
            }
        }
    }

    struct SettingsGuard {
        original: crate::settings::AppSettings,
    }

    impl SettingsGuard {
        fn with_codex_config_dir(dir: Option<&str>) -> Self {
            let original = crate::settings::get_settings();
            let mut settings = original.clone();
            settings.codex_config_dir = dir.map(str::to_string);
            crate::settings::update_settings(settings).unwrap();
            Self { original }
        }
    }

    impl Drop for SettingsGuard {
        fn drop(&mut self) {
            let _ = crate::settings::update_settings(self.original.clone());
        }
    }

    #[test]
    fn get_codex_config_dir_respects_codex_home_env_var_when_directory_exists() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-home-env-home")));
        let _settings = SettingsGuard::with_codex_config_dir(None);
        let codex_home =
            std::env::temp_dir().join(format!("cc-switch-codex-home-env-{}", std::process::id()));
        fs::create_dir_all(&codex_home).unwrap();
        let _env = CodexHomeEnvGuard::new(codex_home.to_str());

        assert_eq!(get_codex_config_dir(), codex_home);

        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_falls_back_to_home_dot_codex_when_codex_home_unset() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-default-home")));
        let _settings = SettingsGuard::with_codex_config_dir(None);
        let _env = CodexHomeEnvGuard::new(None);

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-default-home").join(".codex")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_blank_codex_home_uses_settings_override() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-blank-env-home")));
        let _settings = SettingsGuard::with_codex_config_dir(Some("/tmp/codex-settings-dir"));
        let _env = CodexHomeEnvGuard::new(Some("   "));

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-settings-dir")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_nonexistent_codex_home_uses_settings_override() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-nonexistent-env-home")));
        let _settings = SettingsGuard::with_codex_config_dir(Some("/tmp/codex-settings-dir"));
        let missing = std::env::temp_dir().join(format!(
            "cc-switch-codex-missing-env-{}",
            std::process::id()
        ));
        let _env = CodexHomeEnvGuard::new(missing.to_str());

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-settings-dir")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_file_codex_home_falls_back_to_home_dot_codex() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-file-env-home")));
        let _settings = SettingsGuard::with_codex_config_dir(None);
        let codex_home_file = std::env::temp_dir().join(format!(
            "cc-switch-codex-home-env-file-{}",
            std::process::id()
        ));
        fs::write(&codex_home_file, "not a directory").unwrap();
        let _env = CodexHomeEnvGuard::new(codex_home_file.to_str());

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-file-env-home").join(".codex")
        );

        let _ = fs::remove_file(codex_home_file);
        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_settings_override_takes_precedence_over_codex_home() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-precedence-home")));
        let _settings = SettingsGuard::with_codex_config_dir(Some("/tmp/codex-settings-dir"));
        let codex_home = std::env::temp_dir().join(format!(
            "cc-switch-codex-precedence-env-{}",
            std::process::id()
        ));
        fs::create_dir_all(&codex_home).unwrap();
        let _env = CodexHomeEnvGuard::new(codex_home.to_str());

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-settings-dir")
        );

        let _ = fs::remove_dir_all(codex_home);
        set_test_home_override(None);
    }

    #[test]
    fn prepare_provider_live_config_writes_provider_scoped_bearer_token() {
        let input = r#"model_provider = "vendor_alpha"
model = "gpt-5.4"

[model_providers.vendor_alpha]
name = "Vendor Alpha"
base_url = "https://alpha.example/v1"
wire_api = "responses"
"#;

        let result =
            prepare_codex_provider_live_config(&json!({"OPENAI_API_KEY": "sk-test"}), input)
                .expect("prepare live config");
        let parsed: toml::Value = toml::from_str(&result).expect("parse prepared config");

        assert_eq!(
            parsed
                .get("model_providers")
                .and_then(|v| v.get("vendor_alpha"))
                .and_then(|v| v.get("experimental_bearer_token"))
                .and_then(|v| v.as_str()),
            Some("sk-test")
        );
        assert_eq!(
            extract_codex_experimental_bearer_token(&result).as_deref(),
            Some("sk-test")
        );
    }

    #[test]
    fn restore_backfill_moves_bearer_token_back_to_auth() {
        let mut live_settings = json!({
            "auth": {
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "oauth-access"
                }
            },
            "config": r#"model_provider = "vendor_alpha"
model = "gpt-5.4"

[model_providers.vendor_alpha]
name = "Vendor Alpha"
base_url = "https://alpha.example/v1"
wire_api = "responses"
experimental_bearer_token = "sk-live"
"#
        });
        let template_settings = json!({
            "auth": {
                "OPENAI_API_KEY": "sk-template"
            }
        });

        restore_codex_settings_for_backfill(&mut live_settings, &template_settings, true)
            .expect("restore settings");
        assert_eq!(
            live_settings
                .get("auth")
                .and_then(|auth| auth.get("OPENAI_API_KEY"))
                .and_then(Value::as_str),
            Some("sk-live")
        );
        let config_text = live_settings
            .get("config")
            .and_then(Value::as_str)
            .expect("config text");
        assert!(
            !config_text.contains("experimental_bearer_token"),
            "stored provider config should not keep live bearer tokens"
        );
    }

    #[test]
    fn should_not_restore_provider_token_for_oauth_only_template() {
        let oauth_template = json!({
            "auth": {
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "oauth-access"
                }
            }
        });
        let api_key_template = json!({
            "auth": {
                "OPENAI_API_KEY": "sk-test"
            }
        });

        assert!(
            !should_restore_codex_provider_token_for_backfill(Some("custom"), &oauth_template),
            "OAuth-only templates should not backfill bearer tokens into OPENAI_API_KEY"
        );
        assert!(
            should_restore_codex_provider_token_for_backfill(Some("custom"), &api_key_template),
            "custom API-key providers should still restore provider bearer tokens"
        );
        assert!(
            !should_restore_codex_provider_token_for_backfill(Some("official"), &api_key_template),
            "official providers should never restore third-party bearer tokens"
        );
    }
}
