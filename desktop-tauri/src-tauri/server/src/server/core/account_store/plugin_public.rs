use serde_json::{Map, Value};

use super::state::StoredAccount;

const EXPIRY_KEYS: &[&str] = &[
    "expiresAt", "tokenExpiresAt", "expiresAtMs", "tokenExpireAt", "expires_at", "expiration",
];

pub(super) fn credential_fields(record: &StoredAccount, public: &mut Map<String, Value>) {
    let secret = record.get("pluginSecret").and_then(Value::as_object);
    let has_refresh = ["hasRefreshToken", "canRefresh"].iter().any(|key| {
        public.get(*key).and_then(Value::as_bool) == Some(true)
    }) || ["refreshToken", "refresh_token"].iter().any(|key| {
        [secret.and_then(|fields| fields.get(*key)), record.get(key)]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|token| !token.trim().is_empty())
    });
    public.insert("hasRefreshToken".to_string(), Value::Bool(has_refresh));
    public.insert("canRefresh".to_string(), Value::Bool(has_refresh));
    if record.provider() == super::CATPAW_PROVIDER_ID {
        let token = match secret {
            Some(fields) => fields.get("balanceToken"),
            None => record.get("balanceToken"),
        };
        public.insert("hasBalanceToken".to_string(), Value::Bool(token.and_then(Value::as_str)
            .is_some_and(|token| !token.trim().is_empty())));
    }

    // 只派生白名单标量，凭据对象不进入公开视图；陈旧快照不能压住新的到期时刻。
    let expiry = EXPIRY_KEYS.iter().flat_map(|key| {
        [public.get(*key), secret.and_then(|fields| fields.get(*key)), record.get(key)]
    }).flatten().filter_map(expiry_millis).max();
    if let Some(expiry) = expiry {
        for key in ["expiresAt", "tokenExpiresAt", "expiresAtMs", "tokenExpireAt"] {
            public.insert(key.to_string(), Value::from(expiry));
        }
    }
}

pub(super) fn is_private_field(key: &str) -> bool {
    matches!(key,
        "pluginSecret" | "pluginPublic" | "pluginIdentity" | "accessToken" | "refreshToken"
        | "access_token" | "refresh_token" | "apiKey" | "api_key" | "password"
        | "balanceToken" | "access_key_id" | "secret_access_key" | "security_token" | "oauth_context"
    )
}

fn expiry_millis(value: &Value) -> Option<i64> {
    let raw = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let text = text.trim();
            if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
                return (time.timestamp_millis() > 0).then(|| time.timestamp_millis());
            }
            text.parse::<f64>().ok()
        }
        _ => None,
    }?;
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    let millis = if raw < 1e11 { raw * 1000.0 } else { raw };
    if millis >= i64::MAX as f64 {
        return None;
    }
    (millis >= 1.0).then_some(millis as i64)
}
