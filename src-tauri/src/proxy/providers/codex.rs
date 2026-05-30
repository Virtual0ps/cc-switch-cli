use reqwest::RequestBuilder;

use crate::{provider::Provider, proxy::error::ProxyError};

use super::{AuthInfo, AuthStrategy, ProviderAdapter};

pub struct CodexAdapter;

impl CodexAdapter {
    pub fn new() -> Self {
        Self
    }

    fn extract_key(&self, provider: &Provider) -> Option<String> {
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(key) = env
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|key| !key.is_empty())
            {
                return Some(key.to_string());
            }
        }

        if let Some(auth) = provider.settings_config.get("auth") {
            if let Some(key) = crate::codex_config::extract_codex_auth_api_key(auth) {
                return Some(key);
            }
        }

        if let Some(key) = provider
            .settings_config
            .get("apiKey")
            .or_else(|| provider.settings_config.get("api_key"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|key| !key.is_empty())
        {
            return Some(key.to_string());
        }

        if let Some(config) = provider.settings_config.get("config") {
            if let Some(key) = config
                .get("api_key")
                .or_else(|| config.get("apiKey"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|key| !key.is_empty())
            {
                return Some(key.to_string());
            }

            if let Some(config_str) = config.as_str() {
                if let Some(key) =
                    crate::codex_config::extract_codex_experimental_bearer_token(config_str)
                {
                    return Some(key);
                }
            }
        }

        None
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "Codex"
    }

    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError> {
        if let Some(url) = provider
            .settings_config
            .get("base_url")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        if let Some(url) = provider
            .settings_config
            .get("baseURL")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        if let Some(config) = provider.settings_config.get("config") {
            if let Some(url) = config.get("base_url").and_then(|v| v.as_str()) {
                return Ok(url.trim_end_matches('/').to_string());
            }

            if let Some(config_str) = config.as_str() {
                if let Some(start) = config_str.find("base_url = \"") {
                    let rest = &config_str[start + 12..];
                    if let Some(end) = rest.find('"') {
                        return Ok(rest[..end].trim_end_matches('/').to_string());
                    }
                }
                if let Some(start) = config_str.find("base_url = '") {
                    let rest = &config_str[start + 12..];
                    if let Some(end) = rest.find('\'') {
                        return Ok(rest[..end].trim_end_matches('/').to_string());
                    }
                }
            }
        }

        Err(ProxyError::ConfigError(
            "Codex Provider 缺少 base_url 配置".to_string(),
        ))
    }

    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo> {
        self.extract_key(provider)
            .map(|key| AuthInfo::new(key, AuthStrategy::Bearer))
    }

    fn build_url(&self, base_url: &str, endpoint: &str) -> String {
        let base_trimmed = base_url.trim_end_matches('/');
        let endpoint_trimmed = endpoint.trim_start_matches('/');
        let already_has_v1 = base_trimmed.ends_with("/v1");
        let origin_only = match base_trimmed.split_once("://") {
            Some((_scheme, rest)) => !rest.contains('/'),
            None => !base_trimmed.contains('/'),
        };

        let mut url = if already_has_v1 {
            format!("{base_trimmed}/{endpoint_trimmed}")
        } else if origin_only {
            format!("{base_trimmed}/v1/{endpoint_trimmed}")
        } else {
            format!("{base_trimmed}/{endpoint_trimmed}")
        };

        while url.contains("/v1/v1") {
            url = url.replace("/v1/v1", "/v1");
        }

        url
    }

    fn add_auth_headers(&self, request: RequestBuilder, auth: &AuthInfo) -> RequestBuilder {
        request.header("Authorization", format!("Bearer {}", auth.api_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_provider(settings_config: serde_json::Value) -> Provider {
        Provider::with_id(
            "test".to_string(),
            "Test Provider".to_string(),
            settings_config,
            None,
        )
    }

    #[test]
    fn test_extract_auth_falls_back_to_config_bearer_when_auth_key_empty() {
        let adapter = CodexAdapter::new();
        let provider = create_provider(json!({
            "auth": {
                "OPENAI_API_KEY": ""
            },
            "config": r#"model_provider = "custom"

[model_providers.custom]
experimental_bearer_token = "sk-config-key"
"#
        }));

        let auth = adapter.extract_auth(&provider).expect("extract auth");
        assert_eq!(auth.api_key, "sk-config-key");
        assert_eq!(auth.strategy, AuthStrategy::Bearer);
    }

    #[test]
    fn test_extract_auth_ignores_blank_keys() {
        let adapter = CodexAdapter::new();
        let provider = create_provider(json!({
            "env": {
                "OPENAI_API_KEY": "   "
            },
            "auth": {
                "OPENAI_API_KEY": "\t"
            },
            "apiKey": "",
            "config": {
                "api_key": "  "
            }
        }));

        assert!(adapter.extract_auth(&provider).is_none());
    }
}
