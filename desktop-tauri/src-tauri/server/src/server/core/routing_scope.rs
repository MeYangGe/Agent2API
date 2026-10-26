//! 全局顺序来自账号库与消耗规则；只有模型独立配置覆盖顺序。

use std::collections::HashMap;

use serde_json::Value;

use crate::server::config::{self, RoutingProfile};
use super::{account_order, routing};

pub fn providers_for(model: &str, available: &[String]) -> Vec<String> {
    let profiles = config::routing_profiles();
    match profiles.effective(model) {
        Some(profile) => {
            let mut providers = if profiles.model_profile(model).is_some() {
                profile.providers.clone()
            } else {
                available.to_vec()
            };
            providers.retain(|id| available.contains(id) && profile.providers.contains(id)
                && !profile.disabled_providers.contains(id));
            providers
        }
        None => available.to_vec(),
    }
}

pub fn apply_accounts(accounts: &[Value], model: &str) -> Vec<Value> {
    let profiles = config::routing_profiles();
    let mut ordered = if let Some(profile) = profiles.model_profile(model) {
        project_accounts(accounts, profile)
    } else {
        let sorting = account_order::sorting(accounts, crate::server::logging::now_ms());
        let mut ordered = account_order::ordered_accounts(accounts, &sorting);
        if let Some(profile) = &profiles.global {
            ordered.retain(|account| profile.providers.iter().any(|id| id == routing::provider_of(account))
                && !profile.disabled_providers.iter().any(|id| id == routing::provider_of(account))
                && !routing::account_id(account).is_some_and(|id| profile.excluded_accounts.iter().any(|excluded| excluded == id)));
        }
        ordered
    };
    for (index, account) in ordered.iter_mut().enumerate() {
        if let Some(object) = account.as_object_mut() {
            object.insert("routingPriority".to_string(), Value::from(index));
        }
    }
    ordered
}

pub fn project_accounts(accounts: &[Value], profile: &RoutingProfile) -> Vec<Value> {
    let sorting = account_order::sorting(accounts, crate::server::logging::now_ms());
    let baseline = account_order::ordered_accounts(accounts, &sorting);
    let mut ordered = Vec::new();
    for provider in &profile.providers {
        if profile.disabled_providers.contains(provider) { continue; }
        let mut members: Vec<Value> = baseline.iter()
            .filter(|account| routing::provider_of(account) == provider)
            .filter(|account| routing::account_id(account)
                .is_some_and(|id| !profile.excluded_accounts.iter().any(|excluded| excluded == id)))
            .cloned().collect();
        if let Some(ids) = profile.account_order.get(provider).or_else(|| sorting.account_order.get(provider)) {
            let positions: HashMap<&str, usize> = ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
            members.sort_by_key(|account| routing::account_id(account)
                .and_then(|id| positions.get(id)).copied().unwrap_or(usize::MAX));
        }
        ordered.extend(members);
    }
    ordered
}

pub fn materialize_profile(accounts: &[Value], available: &[String], model: &str) -> RoutingProfile {
    let profiles = config::routing_profiles();
    let sorting = account_order::sorting(accounts, crate::server::logging::now_ms());
    let independent = profiles.model_profile(model).is_some();
    let mut profile = profiles.effective(model).cloned().unwrap_or_else(|| RoutingProfile {
        providers: available.to_vec(),
        ..Default::default()
    });
    if !independent {
        let mut providers = sorting.provider_order.clone();
        providers.extend(available.iter().filter(|id| !sorting.provider_order.contains(id)).cloned());
        for id in &profile.providers {
            if !providers.contains(id) { providers.push(id.clone()); }
        }
        providers.retain(|id| profile.providers.contains(id));
        profile.providers = providers;
        profile.account_order.clear();
    }
    if !independent && !model.is_empty() {
        profile.providers.retain(|id| available.contains(id));
        profile.disabled_providers.retain(|id| profile.providers.contains(id));
        profile.account_order.retain(|provider, _| profile.providers.contains(provider));
    }
    for provider in &profile.providers {
        let ids = profile.account_order.entry(provider.clone()).or_default();
        for id in sorting.account_order.get(provider).into_iter().flatten() {
            if !ids.contains(id) { ids.push(id.clone()); }
        }
    }
    profile
}
