use std::path::PathBuf;

use axum::body::Bytes;
use axum::response::Response;
use base64::Engine;
use serde_json::Value;

use crate::server::core::{sub_kernel, subscriptions};
use crate::server::core::sub_kernel::import::{self as kernel_import, Source};
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};

pub(crate) const MAX_UPLOAD_BYTES: usize = kernel_import::MAX_UPLOAD_BYTES;

pub(super) async fn import(body: &Bytes) -> Response {
    let permit = match kernel_import::reserve() {
        Ok(permit) => permit,
        Err(error) => return management_error(409, error),
    };
    let payload = match parse_body(body) {
        Ok(payload) if payload.is_object() => payload,
        _ => return management_error(400, "请求内容必须是有效 JSON 对象"),
    };
    let replace = payload.get("replace").and_then(Value::as_bool) == Some(true);
    if sub_kernel::installed() && !replace {
        return management_error(409, "已有内核安装，请确认替换后再导入；订阅与账号引用会保留");
    }
    let dir = match directory(&payload) {
        Ok(dir) => dir,
        Err(error) => return management_error(400, error),
    };
    let source = match source(&payload) {
        Ok(source) => source,
        Err(error) => return management_error(400, error),
    };
    match sub_kernel::request_import(source, dir, replace, subscriptions::enabled_provider_inputs(), permit) {
        Ok(_) => ok_json(sub_kernel::status_json()),
        Err(error) => management_error(503, error),
    }
}

fn directory(payload: &Value) -> Result<PathBuf, String> {
    let raw = match payload.get("dir") {
        None | Some(Value::Null) => return Ok(sub_kernel::kernel_dir()),
        Some(Value::String(text)) => text.trim(),
        _ => return Err("存放目录必须是字符串".to_string()),
    };
    if raw.is_empty() { return Ok(sub_kernel::default_kernel_dir()); }
    let path = PathBuf::from(raw);
    if !path.is_absolute() { return Err("存放目录必须是网关机器上的绝对路径".to_string()); }
    if path.exists() && !path.is_dir() { return Err("存放位置是文件，请填写目录路径".to_string()); }
    Ok(path)
}

fn source(payload: &Value) -> Result<Source, String> {
    let encoded = text(payload, "fileB64")?;
    let path = text(payload, "path")?;
    if encoded.is_some() == path.is_some() {
        return Err("请选择上传内核文件，或填写网关机器上的文件／目录路径，不能同时指定".to_string());
    }
    if let Some(path) = path {
        let path = PathBuf::from(path);
        if !path.is_absolute() { return Err("内核文件或目录必须是网关机器上的绝对路径".to_string()); }
        return Ok(Source::Path(path));
    }
    let encoded = encoded.unwrap_or("");
    let encoded = if encoded.starts_with("data:") {
        encoded.split_once("base64,").map(|(_, value)| value)
            .ok_or_else(|| "内核文件内容不是有效 base64".to_string())?
    } else { encoded };
    if encoded.len() > kernel_import::MAX_BASE64_BYTES { return Err(kernel_import::size_error()); }
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)
        .map_err(|_| "内核文件内容不是有效 base64".to_string())?;
    if bytes.is_empty() || bytes.len() > kernel_import::MAX_FILE_BYTES { return Err(kernel_import::size_error()); }
    Ok(Source::Upload(bytes))
}

fn text<'a>(payload: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match payload.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok((!text.trim().is_empty()).then(|| text.trim())),
        _ => Err(format!("{key} 必须是字符串")),
    }
}
