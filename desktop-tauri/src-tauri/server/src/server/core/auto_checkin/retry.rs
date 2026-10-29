//! 中性未领取的**重试记账**（`lastResult.retryAccounts`）+ 补签轮的目标集合。
//!
//! ── 两件不同的事，别混 ──────────────────────────────────────
//!   1. **等窗口重试**（本模块的 `due_accounts` / `merge`）：上一轮拿到「中性
//!      未领取」的账号，等插件给出的 `retryAt`（或条目配置的间隔）到了再签一次。
//!      它解决的是「窗口没开 / 上游一时没下发」，与「今天还没签过」无关。
//!   2. **补签轮**（[`unsigned_accounts`]）：今天到点该签的账号里，`checkinAt`
//!      不在今天的那些 —— 主轮因风控 / 网络抖动整批失败时，这一轮把它们捞回来。
//!
//! ── 为什么补签轮不靠 retryAccounts ─────────────────────────
//! `retryAccounts` 只记「上游说没活动」的账号；整批 429 / 网络失败的行是
//! **failed**，不进那张表（见 `retry_at` 的前两条 return）。而补签轮要覆盖的
//! 恰恰是这批 —— 判据只能是「今天签上了没有」这个账号级事实。
//!
//! ── 间隔与次数可配 ──────────────────────────────────────────
//! `RETRY_GAP_MS` 现在是缺省值；条目上的 `retryGapMinutes` / `retryLimit` 会覆盖
//! 它（见 `checkin_schedules::Schedule`）。`retryLimit = 0` 或
//! `retryGapMinutes = 0` 都表示「不重试」——用户明确要求「别反复打上游」时要能
//! 关掉，而改造前那是一个写死的 10 分钟。

use serde_json::Value;

use crate::server::core::checkin_schedules::Schedule;
use crate::server::core::{account_store::AccountStore, beijing, billing::checkin};

/// 缺省重试间隔（条目没配时用它）
const RETRY_GAP_MS: i64 = 10 * 60 * 1000;

/// 今天该重试的账号（`None` = 没有可重试的）。
///
/// `previous` 是条目上一轮的 `lastResult`；`gap_ms` / `limit` 来自条目配置
/// （见 `checkin_schedules::Schedule` 的 `retryGapMinutes` / `retryLimit`）。
/// 只有**当天**的记账算数：跨天的旧记账一律作废（否则第二天零点会替昨天的账号
/// 再打一遍上游）。
pub(super) fn due_accounts_scoped(
    previous: Option<&Value>,
    now: i64,
    gap_ms: i64,
    limit: i64,
) -> Option<Vec<String>> {
    if gap_ms <= 0 || limit <= 0 {
        return None;
    }
    let previous = previous.filter(|value| {
        value.get("date").and_then(Value::as_str) == Some(beijing::today_key().as_str())
    })?;
    let retry = previous.get("retryAccounts").and_then(Value::as_object)?;
    if retry.is_empty() {
        return None;
    }
    Some(
        retry
            .iter()
            .filter(|(_, at)| at.as_i64().is_some_and(|at| at <= now))
            .map(|(id, _)| id.clone())
            .collect(),
    )
}

/// 条目的重试间隔（毫秒）：`retryGapMinutes` 覆盖缺省；0 = 关掉重试。
pub(super) fn gap_ms_of(schedule: Option<&Schedule>) -> i64 {
    match schedule {
        Some(schedule) if schedule.retry_gap_minutes <= 0 => 0,
        Some(schedule) => schedule.retry_gap_minutes.saturating_mul(60_000),
        None => RETRY_GAP_MS,
    }
}

/// 条目的重试次数上限：0 = 关掉重试。
pub(super) fn limit_of(schedule: Option<&Schedule>) -> i64 {
    schedule.map(|schedule| schedule.retry_limit).unwrap_or(3)
}

fn retry_at(store: &AccountStore, row: &Value, now: i64, gap_ms: i64) -> Option<i64> {
    if row
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
        || checkin::day_completed(row)
    {
        return None;
    }
    let claim = row.get("claim")?;
    if !checkin::claim_pending(claim) {
        return None;
    }
    if let Some(at) = claim.get("retryAt") {
        return at
            .as_i64()
            .filter(|at| *at > 0)
            .map(|at| at.max(now + 30_000));
    }
    let id = row.get("id").and_then(Value::as_str)?;
    let provider = store.account_provider_of(id)?;
    let qoder = crate::server::core::providers::qoder::endpoints::Region::Global.provider_id();
    if provider == qoder {
        // Qoder 国际版的窗口由插件在 `retryAt` 里给出（10:00 那个点）；这里
        // 只是它没给时的兜底：等到当天 10:00，与 `providers::qoder::checkin`
        // 的 `daily_window_start` 同一口径。
        let window = beijing::now()
            .date_naive()
            .and_hms_opt(10, 0, 0)?
            .and_local_timezone(beijing::offset())
            .single()?
            .timestamp_millis();
        return Some(window.max(now + gap_ms));
    }
    (provider == "codearts").then_some(now + gap_ms)
}

pub(super) fn merge(
    store: &AccountStore,
    mut summary: Value,
    result: &Value,
    previous: Option<&Value>,
    retried: Option<&[String]>,
    gap_ms: i64,
) -> Value {
    let Some(fields) = summary.as_object_mut() else {
        return summary;
    };
    let now = crate::server::logging::now_ms();
    // ── 重试名单：**始终**从当天的上一轮继承 ────────────────────
    // 一条条目一天会跑多批（各家时刻不同），每一批只覆盖自己那几家。若每批都
    // 从空表重建，上一批记下的「等窗口重试」就会被冲掉（CodeArts 那类靠它重试
    // 的账号会静默失去重试机会）。所以基线永远是当天的旧表，只把本轮真跑过的
    // 那几个 id 摘掉。
    let mut retry = previous
        .filter(|value| value.get("date").and_then(Value::as_str) == Some(beijing::today_key().as_str()))
        .and_then(|value| value.get("retryAccounts"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(retried) = retried {
        for id in retried {
            retry.remove(id);
        }
    }
    if let (Some(previous), Some(_)) = (previous, retried) {
        for key in ["succeeded", "active", "failedCount"] {
            let before = previous.get(key).and_then(Value::as_u64).unwrap_or(0);
            let current = fields.get(key).and_then(Value::as_u64).unwrap_or(0);
            fields.insert(key.to_string(), Value::from(before.saturating_add(current)));
        }
        for key in ["total", "skipped"] {
            if let Some(value) = previous.get(key) {
                fields.insert(key.to_string(), value.clone());
            }
        }
        let mut failures = previous
            .get("failed")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        failures.extend(
            fields
                .get("failed")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        failures.truncate(5);
        fields.insert("failed".to_string(), Value::Array(failures));
    }
    if let Some(rows) = result.get("results").and_then(Value::as_array) {
        for row in rows {
            let Some(id) = row.get("id").and_then(Value::as_str) else {
                continue;
            };
            // 本轮真跑过的账号一律先从旧表摘掉，再由 `retry_at` 决定要不要放回 ——
            // 否则「上次在等窗口、这次已经签上」的账号会留在表里被反复重试。
            // （改造前每轮都是全量范围，`retried` 覆盖得住；现在按家分批，
            // 主轮那次 `retried` 是 None，必须靠这里兜。）
            retry.remove(id);
            if let Some(at) = retry_at(store, row, now, gap_ms) {
                retry.insert(id.to_string(), Value::from(at));
            }
        }
    }
    fields.insert("retryAccounts".to_string(), Value::Object(retry));
    summary
}

pub(super) fn next_at(result: Option<&Value>, today: &str) -> Option<i64> {
    let result = result.filter(|value| value.get("date").and_then(Value::as_str) == Some(today))?;
    result
        .get("retryAccounts")
        .and_then(Value::as_object)?
        .values()
        .filter_map(Value::as_i64)
        .min()
}

/// **今天还没签上的账号 id**（补签轮的目标集合）。
///
/// 判据是账号的 `checkinAt` 是否落在今天（北京时间）—— 与账号页「今日已签到」、
/// 签到中心的 `checkedInToday` 同一份事实，所以三处永远不会给出矛盾结论。
///
/// 不看 `available` / `enabled`：那两个管的是转发，与签到无关（见
/// `billing::checkin` 模块头「签到不看 enabled」）。真正该不该签由
/// `resolve_checkin_targets` 的 `supports_checkin` 与范围过滤决定。
pub(super) fn unsigned_accounts(store: &AccountStore) -> Vec<String> {
    let today = beijing::today_key();
    store
        .list_accounts()
        .get("accounts")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|account| {
                    let at = account.get("checkinAt").and_then(Value::as_i64).unwrap_or(0);
                    !beijing::date_key_of_ms(at).is_some_and(|key| key == today)
                })
                .filter_map(|account| account.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
