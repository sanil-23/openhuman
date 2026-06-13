//! Detect Claude Code CLI auth state via the CLI's own structured probe.
//!
//! Resolution order:
//!   1. `ANTHROPIC_API_KEY` env var present → `ApiKeyEnv` (the spawned CLI
//!      inherits it, so it wins regardless of any logged-in session).
//!   2. `claude auth status --json` → parse the structured result.
//!
//! Why spawn the CLI instead of reading `~/.claude/.credentials.json`:
//! on **macOS** the `claude` CLI stores credentials in the Keychain
//! (service `Claude Code-credentials`), *not* in that file. A logged-in
//! macOS user therefore had no credentials file and was misreported as
//! signed out — on the primary shipping platform. `claude auth status`
//! abstracts file-vs-Keychain for us and works on macOS, Linux, and
//! Windows. It returns only non-secret metadata (login flag, auth method,
//! account email, subscription type) — never the access/refresh token —
//! so we keep the "never read the token" principle.
//!
//! Older CLIs (below the `auth status` cut) yield a non-zero exit or
//! non-JSON output; we map those to [`AuthSource::Unknown`] and never to
//! `None`, so we never tell a signed-in user they're signed out just
//! because their binary predates the subcommand.

use std::process::Command;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::version_check;

/// Discriminator for who actually authenticates the spawned CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "source")]
pub enum AuthSource {
    /// Claude Pro / Max subscription — `claude auth status` reports
    /// `loggedIn: true` with `authMethod: "claude.ai"`. Account email +
    /// subscription type returned best-effort; absent when the schema
    /// drifts.
    Subscription {
        account_email: Option<String>,
        /// `"max"` / `"pro"` etc., for display. Absent when not reported.
        subscription_type: Option<String>,
        /// Reserved for a future expiry field; `claude auth status` does
        /// not currently expose one, so this is always `None` today. Kept
        /// for wire stability with the prior schema.
        expires_at: Option<String>,
    },
    /// `ANTHROPIC_API_KEY` is set in the core process env, or the CLI is
    /// logged in via an API key rather than a claude.ai subscription. The
    /// spawned CLI authenticates with that key.
    ApiKeyEnv,
    /// `claude auth status` reported `loggedIn: false`. The CLI will fail
    /// any chat with an auth error until the user signs in.
    None,
    /// Could not determine the sign-in state — binary missing, spawn
    /// failed, non-zero exit (e.g. a CLI older than `auth status`), or
    /// unparseable output. We surface this as "couldn't determine" and a
    /// Reconnect affordance, **never** as signed-out.
    Unknown { reason: Option<String> },
}

/// Returned by the `claude_code_auth_status` RPC. Snake-case Serde so the
/// TS side discriminates on `source`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatus {
    #[serde(flatten)]
    pub source: AuthSource,
    /// Unix seconds when this probe ran — UI shows "last checked" so users
    /// can tell a stale subscription badge from a fresh one.
    pub last_checked: u64,
}

/// Parse the stdout of `claude auth status --json` into an [`AuthSource`].
///
/// Pure function — no spawn, no env — so it is unit-testable. Observed
/// shape (claude 2.1.175):
/// ```json
/// {
///   "loggedIn": true,
///   "authMethod": "claude.ai",
///   "apiProvider": "firstParty",
///   "email": "user@example.com",
///   "subscriptionType": "max"
/// }
/// ```
pub fn parse_auth_status_json(raw: &str) -> AuthSource {
    let val: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return AuthSource::Unknown {
                reason: Some(format!("unparseable `claude auth status` JSON: {e}")),
            };
        }
    };

    // The single field we treat as load-bearing. If it's absent the shape
    // is foreign to us — refuse to guess, return Unknown.
    let logged_in = match val.get("loggedIn").and_then(serde_json::Value::as_bool) {
        Some(b) => b,
        None => {
            return AuthSource::Unknown {
                reason: Some("`claude auth status` JSON missing `loggedIn`".to_string()),
            };
        }
    };

    if !logged_in {
        return AuthSource::None;
    }

    let auth_method = val
        .get("authMethod")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // `authMethod: "claude.ai"` is the Pro/Max OAuth subscription. Any
    // other logged-in method (console API key, etc.) authenticates via a
    // key, so we surface it like an API-key source rather than inventing a
    // subscription badge.
    if auth_method == "claude.ai" {
        let account_email = val
            .get("email")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let subscription_type = val
            .get("subscriptionType")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        AuthSource::Subscription {
            account_email,
            subscription_type,
            expires_at: None,
        }
    } else {
        AuthSource::ApiKeyEnv
    }
}

/// Spawn `claude auth status --json` and classify the result. Honors the
/// `OPENHUMAN_CLAUDE_CLI` override via [`version_check::resolve_binary`].
fn probe_via_cli() -> AuthSource {
    let Some(bin) = version_check::resolve_binary() else {
        log::debug!("[claude-code][auth] no `claude` binary on PATH; auth state unknown");
        return AuthSource::Unknown {
            reason: Some("`claude` CLI not found on PATH".to_string()),
        };
    };

    let bin_str = bin.display().to_string();
    let output = match Command::new(&bin)
        .args(["auth", "status", "--json"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log::warn!("[claude-code][auth] spawn failed bin={bin_str} err={e}");
            return AuthSource::Unknown {
                reason: Some(format!("spawn failed: {e}")),
            };
        }
    };

    if !output.status.success() {
        // Most likely an older CLI without the `auth status` subcommand.
        // Never downgrade to signed-out on this path.
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::debug!(
            "[claude-code][auth] `claude auth status` exit={} stderr={}",
            output.status,
            stderr.trim()
        );
        return AuthSource::Unknown {
            reason: Some(format!("`claude auth status` exited {}", output.status)),
        };
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let source = parse_auth_status_json(stdout.trim());
    log::debug!(
        "[claude-code][auth] probe classified source={}",
        match &source {
            AuthSource::Subscription { .. } => "subscription",
            AuthSource::ApiKeyEnv => "api_key_env",
            AuthSource::None => "none",
            AuthSource::Unknown { .. } => "unknown",
        }
    );
    source
}

/// Probe auth state. Spawns the `claude` CLI (same cost class as the
/// `claude --version` probe) — keep call-sites on-demand, never on a hot
/// path.
pub fn probe() -> AuthStatus {
    let last_checked = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        if !k.trim().is_empty() {
            log::debug!("[claude-code][auth] ANTHROPIC_API_KEY present → api_key_env");
            return AuthStatus {
                source: AuthSource::ApiKeyEnv,
                last_checked,
            };
        }
    }

    AuthStatus {
        source: probe_via_cli(),
        last_checked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_ai_subscription() {
        let raw = r#"{
            "loggedIn": true,
            "authMethod": "claude.ai",
            "apiProvider": "firstParty",
            "email": "user@example.com",
            "subscriptionType": "max"
        }"#;
        match parse_auth_status_json(raw) {
            AuthSource::Subscription {
                account_email,
                subscription_type,
                expires_at,
            } => {
                assert_eq!(account_email.as_deref(), Some("user@example.com"));
                assert_eq!(subscription_type.as_deref(), Some("max"));
                assert!(expires_at.is_none());
            }
            other => panic!("expected Subscription, got {other:?}"),
        }
    }

    #[test]
    fn subscription_tolerates_missing_email_and_type() {
        let raw = r#"{ "loggedIn": true, "authMethod": "claude.ai" }"#;
        match parse_auth_status_json(raw) {
            AuthSource::Subscription {
                account_email,
                subscription_type,
                ..
            } => {
                assert!(account_email.is_none());
                assert!(subscription_type.is_none());
            }
            other => panic!("expected Subscription, got {other:?}"),
        }
    }

    #[test]
    fn logged_in_via_api_key_method_maps_to_api_key_env() {
        let raw = r#"{ "loggedIn": true, "authMethod": "console", "apiProvider": "console" }"#;
        assert_eq!(parse_auth_status_json(raw), AuthSource::ApiKeyEnv);
    }

    #[test]
    fn logged_out_maps_to_none() {
        let raw = r#"{ "loggedIn": false }"#;
        assert_eq!(parse_auth_status_json(raw), AuthSource::None);
    }

    #[test]
    fn missing_logged_in_field_is_unknown() {
        let raw = r#"{ "authMethod": "claude.ai", "email": "user@example.com" }"#;
        match parse_auth_status_json(raw) {
            AuthSource::Unknown { reason } => assert!(reason.is_some()),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_is_unknown_not_signed_out() {
        match parse_auth_status_json("not json at all") {
            AuthSource::Unknown { reason } => assert!(reason.is_some()),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn api_key_env_short_circuits_probe() {
        let _env = super::super::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("ANTHROPIC_API_KEY").ok();
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");

        let s = probe();
        assert_eq!(s.source, AuthSource::ApiKeyEnv);

        match prev {
            Some(v) => std::env::set_var("ANTHROPIC_API_KEY", v),
            None => std::env::remove_var("ANTHROPIC_API_KEY"),
        }
    }
}
