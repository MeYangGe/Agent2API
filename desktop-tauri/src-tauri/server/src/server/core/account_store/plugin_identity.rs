//! 插件账号落库前的身份解析与主键分配。旧账号保留原 ID，新账号按 provider 隔离。

use std::sync::MutexGuard;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::providers::{kind_from_id, ProviderKind};

use super::account_identity;
use super::sql;
use super::state::StoredAccount;
use super::store::AccountStore;

pub(super) fn incoming(provider: &str, identity: &str, secret: &Value, public: &Value, desktop: bool) -> StoredAccount {
    let mut fields = Map::new();
    fields.insert("provider".to_string(), Value::String(provider.to_string()));
    fields.insert("pluginSecret".to_string(), secret.clone());
    fields.insert("pluginPublic".to_string(), public.clone());
    fields.insert("desktop".to_string(), Value::Bool(desktop));
    let mut record = StoredAccount::from_map(fields);
    if (kind_from_id(provider).is_none() || super::is_cline_family(provider)) && !identity.trim().is_empty() {
        record.set(IDENTITY_KEY, Value::String(identity.to_string()));
    }
    record
}

const IDENTITY_KEY: &str = "pluginIdentity";

fn trusted_identity(provider: &str, identity: &str, fields: &Map<String, Value>) -> bool {
    !identity.trim().is_empty() && !account_identity::guest(provider, fields)
        && (kind_from_id(provider).is_none() || account_identity::stable(provider, fields).is_some())
}

pub(super) fn remember_identity(record: &mut StoredAccount, provider: &str, identity: &str) {
    if trusted_identity(provider, identity, record.fields()) {
        record.set(IDENTITY_KEY, Value::String(identity.to_string()));
    } else {
        record.remove(IDENTITY_KEY);
    }
}

fn same_plugin_identity(record: &StoredAccount, provider: &str, identity: &str, fields: &Map<String, Value>) -> bool {
    trusted_identity(provider, identity, fields)
        && (record.get(IDENTITY_KEY).and_then(Value::as_str) == Some(identity) || record.id() == identity)
        && account_identity::reference(provider, record.fields()) == account_identity::reference(provider, fields)
        && !account_identity::conflicts(provider, record.fields(), fields)
}

fn new_id(provider: &str, identity: &str, fields: &Map<String, Value>) -> Result<String, String> {
    let seed = if account_identity::guest(provider, fields) {
        None
    } else {
        account_identity::stable(provider, fields)
            .or_else(|| trusted_identity(provider, identity, fields).then(|| identity.to_string()))
            .or_else(|| account_identity::credential(provider, fields))
    };
    let suffix = match seed {
        Some(seed) => {
            let mut hash = Sha256::new();
            hash.update(provider.as_bytes());
            hash.update([0]);
            hash.update([u8::from(account_identity::reference(provider, fields))]);
            hash.update([0]);
            hash.update(seed.as_bytes());
            format!("{:x}", hash.finalize())
        }
        None => {
            let mut random = [0u8; 16];
            getrandom::getrandom(&mut random)
                .map_err(|_| "无法生成安全的随机账号 id，请重试".to_string())?;
            random.iter().map(|byte| format!("{byte:02x}")).collect()
        }
    };
    Ok(format!("{provider}-account-{suffix}"))
}

fn known_desktop_id(provider: &str, identity: &str, fields: &Map<String, Value>) -> Option<String> {
    if !account_identity::desktop(fields) { return None; }
    let expected = match kind_from_id(provider) {
        Some(ProviderKind::CatPaw) => "desktop-auth".to_string(),
        Some(ProviderKind::Raccoon) => "raccoon-desktop".to_string(),
        Some(ProviderKind::AutoClaw | ProviderKind::AutoClawIntl | ProviderKind::ClineFree | ProviderKind::ClinePass) => {
            format!("{provider}-desktop")
        }
        _ => return None,
    };
    (identity == expected).then_some(expected)
}

impl AccountStore {
    pub(super) fn plugin_account_for_upsert(
        &self,
        guard: &MutexGuard<'_, ()>,
        provider: &str,
        identity: &str,
        account: &StoredAccount,
    ) -> Result<(String, Option<StoredAccount>), String> {
        if let Some(region) = crate::server::core::providers::qoder::endpoints::Region::from_provider_id(provider) {
            let secret = account.get("pluginSecret").unwrap_or(&Value::Null);
            return self.qoder_account_for_upsert(guard, region, identity, secret);
        }
        let records = self.with_conn(guard, |conn| {
            let ids: Vec<String> = sql::queue_rows(conn)?.into_iter()
                .filter(|row| row.provider == provider).map(|row| row.id).collect();
            ids.into_iter().map(|id| sql::load_by_id(conn, &id))
                .collect::<rusqlite::Result<Vec<_>>>()
        }).map_err(|error| error.message)?;
        let mut records: Vec<StoredAccount> = records.into_iter().flatten().collect();
        records.sort_by_key(|record| (record.added_at(), record.order_key(), record.id().to_string()));

        let desktop_id = known_desktop_id(provider, identity, account.fields());
        if let Some(id) = desktop_id.as_deref() {
            if let Some(record) = records.iter().find(|record| record.id() == id) {
                if !account_identity::desktop(record.fields()) {
                    return Err("桌面端账号 ID 已被普通账号占用，未覆盖原账号".to_string());
                }
                return Ok((id.to_string(), Some(record.clone())));
            }
        }
        let occupied = if identity.trim().is_empty() {
            None
        } else {
            self.with_conn(guard, |conn| sql::load_by_id(conn, identity)).map_err(|error| error.message)?
        };
        // 同 ID 仍要核对身份；插件 ID 不带域或地区时不能覆盖已存在的另一条账号。
        if let Some(record) = occupied.as_ref().filter(|record| record.provider() == provider) {
            if account_identity::conflicts(provider, record.fields(), account.fields()) {
                return Err("插件账号 ID 与已有账号的业务身份冲突，未覆盖原账号".to_string());
            }
        }
        let existing = records.into_iter().find(|record| {
            account_identity::same(provider, record.fields(), account.fields())
                || same_plugin_identity(record, provider, identity, account.fields())
        });
        if let Some(record) = existing {
            return Ok((record.id().to_string(), Some(record)));
        }
        let id = match desktop_id {
            Some(id) => id,
            None => new_id(provider, identity, account.fields())?,
        };
        if self.with_conn(guard, |conn| sql::load_by_id(conn, &id)).map_err(|error| error.message)?.is_some() {
            return Err("账号 ID 已被其它账号占用，未覆盖原账号".to_string());
        }
        Ok((id, None))
    }
}
