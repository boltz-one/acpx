//! Ports `others/acpx/src/acp/auth-env.ts`.
//!
//! Builds the environment for a spawned ACP agent subprocess: auth
//! credentials and session-supplied env vars are merged on top of this
//! process's own environment, with a `*_AUTH_<TOKEN>`-prefixed promotion
//! scheme so a credential can be supplied either as a raw env var
//! (`OPENAI_API_KEY`) or a namespaced override (`ACP_AUTH_OPENAI_API_KEY`)
//! without one silently shadowing the other.
//!
//! Security (see phase Security Considerations): none of these functions
//! log credential values, and callers must not `Debug`/`log::debug!` the
//! resulting env map — it contains secrets by construction. Renamed from
//! acpx's `ACPX_AUTH_` prefix to `ACP_AUTH_` since this crate is not the
//! `acpx` CLI.

use std::collections::{HashMap, HashSet};

const AUTH_ENV_PREFIX: &str = "ACP_AUTH_";

/// Ports `toEnvToken`: uppercases `value` and replaces runs of
/// non-alphanumeric characters with a single underscore, trimming leading/
/// trailing underscores.
pub fn to_env_token(value: &str) -> String {
    let mut token = String::new();
    let mut last_was_sep = false;
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            token.push(ch.to_ascii_uppercase());
            last_was_sep = false;
        } else if !last_was_sep && !token.is_empty() {
            token.push('_');
            last_was_sep = true;
        }
    }
    if token.ends_with('_') {
        token.pop();
    }
    token
}

fn auth_env_key(method_id: &str) -> Option<String> {
    let token = to_env_token(method_id);
    (!token.is_empty()).then(|| format!("{AUTH_ENV_PREFIX}{token}"))
}

/// Ports `readEnvCredential`: reads `ACP_AUTH_<TOKEN(method_id)>` from the
/// current process environment.
pub fn read_env_credential(method_id: &str) -> Option<String> {
    let key = auth_env_key(method_id)?;
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// Ports `resolveConfiguredAuthCredential`: looks up `method_id` verbatim,
/// then its normalized token form, in a caller-supplied credentials map
/// (e.g. from app config), before falling back to the environment.
pub fn resolve_configured_auth_credential<'a>(
    method_id: &str,
    auth_credentials: Option<&'a HashMap<String, String>>,
) -> Option<&'a str> {
    let credentials = auth_credentials?;
    credentials
        .get(method_id)
        .or_else(|| credentials.get(&to_env_token(method_id)))
        .map(String::as_str)
}

fn protected_env_key(key: &str, windows: bool) -> String {
    if windows {
        key.to_uppercase()
    } else {
        key.to_string()
    }
}

/// Builds the full environment for the agent subprocess: inherits this
/// process's env, promotes any pre-set `ACP_AUTH_*` vars to their bare
/// token form (without clobbering an already-set bare var), overlays
/// `auth_credentials` (method id -> secret), then overlays `session_env`
/// (skipping any key that collides with a protected auth key). Ports
/// `buildAgentEnvironment`.
pub fn build_agent_environment(
    base_env: impl IntoIterator<Item = (String, String)>,
    auth_credentials: Option<&HashMap<String, String>>,
    session_env: Option<&HashMap<String, String>>,
    windows: bool,
) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = base_env.into_iter().collect();
    let mut protected: HashSet<String> = HashSet::new();

    for (key, value) in env.clone() {
        if !protected_env_key(&key, windows).starts_with(AUTH_ENV_PREFIX) || value.trim().is_empty()
        {
            continue;
        }
        let suffix = &key[AUTH_ENV_PREFIX.len()..];
        let normalized = to_env_token(suffix);
        if normalized.is_empty() {
            continue;
        }
        protected.insert(protected_env_key(&key, windows));
        protected.insert(normalized.clone());
        env.entry(normalized).or_insert(value);
    }

    if let Some(auth_credentials) = auth_credentials {
        for (method_id, credential) in auth_credentials {
            if credential.trim().is_empty() {
                continue;
            }
            if !method_id.contains('=') && !method_id.contains('\0') {
                protected.insert(protected_env_key(method_id, windows));
                env.entry(method_id.clone())
                    .or_insert_with(|| credential.clone());
            }
            let normalized = to_env_token(method_id);
            if !normalized.is_empty() {
                protected.insert(format!("{AUTH_ENV_PREFIX}{normalized}"));
                protected.insert(normalized.clone());
                env.entry(format!("{AUTH_ENV_PREFIX}{normalized}"))
                    .or_insert_with(|| credential.clone());
                env.entry(normalized).or_insert_with(|| credential.clone());
            }
        }
    }

    if let Some(session_env) = session_env {
        for (key, value) in session_env {
            if protected.contains(&protected_env_key(key, windows)) {
                continue;
            }
            let normalized = protected_env_key(key, windows);
            env.retain(|existing_key, _| protected_env_key(existing_key, windows) != normalized);
            env.insert(key.clone(), value.clone());
        }
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_env_token_normalizes_punctuation() {
        assert_eq!(to_env_token(" openai.api-key "), "OPENAI_API_KEY");
    }

    #[test]
    fn build_agent_environment_promotes_prefixed_var() {
        let base = [("ACP_AUTH_OPENAI_API_KEY".to_string(), "secret".to_string())];
        let env = build_agent_environment(base, None, None, false);
        assert_eq!(env.get("OPENAI_API_KEY"), Some(&"secret".to_string()));
    }

    #[test]
    fn build_agent_environment_applies_auth_credentials() {
        let creds = HashMap::from([("openai-api-key".to_string(), "sk-123".to_string())]);
        let env = build_agent_environment([], Some(&creds), None, false);
        assert_eq!(env.get("openai-api-key"), Some(&"sk-123".to_string()));
        assert_eq!(env.get("OPENAI_API_KEY"), Some(&"sk-123".to_string()));
        assert_eq!(
            env.get("ACP_AUTH_OPENAI_API_KEY"),
            Some(&"sk-123".to_string())
        );
    }

    #[test]
    fn session_env_does_not_override_protected_auth_key() {
        let creds = HashMap::from([("TOKEN".to_string(), "secret".to_string())]);
        let session = HashMap::from([("TOKEN".to_string(), "attacker-controlled".to_string())]);
        let env = build_agent_environment([], Some(&creds), Some(&session), false);
        assert_eq!(env.get("TOKEN"), Some(&"secret".to_string()));
    }

    #[test]
    fn resolve_configured_auth_credential_checks_normalized_key() {
        let creds = HashMap::from([("OPENAI_API_KEY".to_string(), "sk-1".to_string())]);
        assert_eq!(
            resolve_configured_auth_credential("openai-api-key", Some(&creds)),
            Some("sk-1")
        );
    }
}
