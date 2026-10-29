//! 插件 provider 的运行时注册表：provider id → `RpcAdapter`。
//!
//! ── 与内置注册表的关系 ──────────────────────────────────────
//! 内置家走 `providers::adapter::adapter_for(kind)`（编译期枚举，O(1) 永不
//! 失败）。插件家没有枚举值 —— 它们是**用户安装的**，装了什么、装了哪个
//! 版本都是运行期事实。所以这张表按字符串查，且查询可能返回 `None`
//! （没装那个包，或者包被停用）。
//!
//! ── 装配时机 ────────────────────────────────────────────────
//! `bootstrap` 里调一次 [`install`]：读注册表 → 对启用的包 spawn 宿主 →
//! 握手 → 为每个 provider 建 `RpcAdapter` 塞进本表。
//!
//! ── 为什么表里存 `Arc` 而不是 `&'static` ────────────────────
//! 插件可以中途卸载/重装（换版本）。`Arc` 让「旧适配器还在被一条在途请求
//! 持有」与「新适配器已就位」能共存 —— 旧请求用旧适配器跑完，新请求用新的。
//! 换成 `&'static` 就只能靠泄漏来满足生命周期，卸载时无法回收。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::rpc::RpcAdapter;
use crate::server::core::providers::adapter::ProviderAdapter;

/// 进程级注册表。
fn table() -> &'static Mutex<HashMap<String, Arc<dyn ProviderAdapter>>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Arc<dyn ProviderAdapter>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 装入一个插件 provider（重复调用会覆盖 —— 重装/升级走这条）。
pub fn install(id: String, adapter: Arc<RpcAdapter>) {
    if crate::server::core::providers::availability::is_suspended(&id) { return; }
    if let Ok(mut map) = table().lock() {
        map.insert(id, adapter);
    }
}

/// 按 id 查（`None` = 这个 id 不由插件提供，或包没装/被停用）。
pub fn lookup(id: &str) -> Option<Arc<dyn ProviderAdapter>> {
    if crate::server::core::providers::availability::is_suspended(id) { return None; }
    table().lock().ok()?.get(id).cloned()
}

/// 这个 id 此刻**是否由已装入的插件接管**。
///
/// ── 为什么单独一个谓词（而不是让调用方自己 `lookup().is_some()`）──
/// 网关里有大量 `kind_from_id(id).is_some()` 形态的判定，它们的语义是
/// 「这是不是内置家」。插件装好之后，同一个 id **两边都有**，那个判定必须
/// 改成「先问插件」。把「问插件」收成一个具名函数有两个好处：
///   1. 调用点读起来是 `if plugin_serves(id) { …插件分支… } else { …内置… }`，
///      而不是一句容易看漏的 `lookup(id).is_some()`；
///   2. 将来若要加「按配置停用某家的插件实现」，只改这一个函数。
pub fn serves(id: &str) -> bool {
    if crate::server::core::providers::availability::is_suspended(id) { return false; }
    match table().lock() {
        Ok(map) => map.contains_key(id),
        Err(_) => false,
    }
}

/// 列出全部已装入的插件 provider id。
pub fn installed_ids() -> Vec<String> {
    match table().lock() {
        Ok(map) => {
            let mut ids: Vec<String> = map.keys()
                .filter(|id| !crate::server::core::providers::availability::is_suspended(id))
                .cloned().collect();
            ids.sort();
            ids
        }
        Err(_) => Vec::new(),
    }
}

/// 移除一个（卸载包 / 停用某家时调）。
pub fn remove(id: &str) {
    if let Ok(mut map) = table().lock() {
        map.remove(id);
    }
    super::rpc_cache::forget(id);
    super::schedules::forget(id);
}

/// 清空（宿主重启前调 —— 适配器要按新的握手结果重建）。
pub fn clear() {
    if let Ok(mut map) = table().lock() {
        map.clear();
    }
    super::schedules::clear_all();
}
