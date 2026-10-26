//! Images 响应聚合：不经 Chat 翻译器，保留 OpenAI data 或 Gemini inlineData。

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{json, Value};

use crate::server::core::protocol::SseLineBuffer;
use crate::server::core::providers::images::ImageResponseKind;
use crate::server::errors::GatewayError;

use super::usage::RequestTelemetry;

pub(super) struct ImageFailure {
    pub status: u16,
    pub body: Value,
}

impl ImageFailure {
    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: 502,
            body: json!({ "message": message.into() }),
        }
    }
}

pub(super) enum ImageResponse {
    Live(reqwest::Response),
    Complete(Value),
}

impl ImageResponse {
    pub fn status(&self) -> reqwest::StatusCode {
        match self {
            Self::Live(response) => response.status(),
            Self::Complete(_) => reqwest::StatusCode::OK,
        }
    }
}

pub(super) async fn collect(
    response: reqwest::Response,
    kind: ImageResponseKind,
    transport: &super::request::TransportRequest,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<Value, ImageFailure> {
    let budget = Duration::from_millis(crate::server::config::timeout_settings().body_ms());
    let reading = collect_body(response, kind, transport, telemetry);
    let timed = async {
        tokio::time::timeout(budget, reading)
            .await
            .unwrap_or_else(|_| {
                Err(ImageFailure::invalid(format!(
                    "生图响应超时（{}秒），上游可能已消耗额度",
                    budget.as_secs()
                )))
            })
    };
    if let Some(token) = telemetry.cancel_token() {
        tokio::select! {
            result = timed => result,
            _ = token.cancelled() => Err(ImageFailure {
                status: super::cancellation::MANUAL_TERMINATED_STATUS as u16,
                body: json!({ "message": super::cancellation::MANUAL_TERMINATED }),
            }),
        }
    } else {
        timed.await
    }
}

async fn collect_body(
    response: reqwest::Response,
    kind: ImageResponseKind,
    transport: &super::request::TransportRequest,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<Value, ImageFailure> {
    const MAX_RESPONSE_BYTES: usize = 96 * 1024 * 1024;
    let status = response.status().as_u16();
    let sse = matches!(kind, ImageResponseKind::Gemini)
        && response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
    let capture = telemetry.capture();
    if let Some(capture) = capture.as_deref() {
        capture.attach_response(status, response.headers());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    let mut lines = SseLineBuffer::new();
    let mut total = 0usize;
    let mut images = Vec::new();
    let mut usage = None;
    let mut finished = false;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| ImageFailure::invalid("上游生图响应传输中断，上游可能已消耗额度"))?;
        total = total.saturating_add(chunk.len());
        if total > MAX_RESPONSE_BYTES {
            return Err(ImageFailure::invalid("上游生图响应超过 96 MiB 上限"));
        }
        if !chunk.is_empty() {
            telemetry.note_first_frame();
        }
        if let Some(capture) = capture.as_deref() {
            capture.push(&chunk);
        }
        if sse {
            for line in lines.push(&chunk) {
                let Some(line) = line else {
                    finished = true;
                    continue;
                };
                let frame: Value = serde_json::from_str(&line)
                    .map_err(|_| ImageFailure::invalid("上游生图 SSE 含不完整的 JSON 帧"))?;
                gemini_frame(&frame, &mut images, &mut usage, &mut finished)?;
            }
        } else {
            bytes.extend_from_slice(&chunk);
        }
    }
    if sse {
        for line in lines.finish() {
            let Some(line) = line else {
                finished = true;
                continue;
            };
            let frame: Value = serde_json::from_str(&line)
                .map_err(|_| ImageFailure::invalid("上游生图 SSE 被截断"))?;
            gemini_frame(&frame, &mut images, &mut usage, &mut finished)?;
        }
        if !finished {
            return Err(ImageFailure::invalid(
                "上游生图流没有正常收尾，未将可能截断的图片当作成功",
            ));
        }
    } else {
        let payload: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ImageFailure::invalid("上游生图响应不是有效 JSON"))?;
        if matches!(kind, ImageResponseKind::ModelscopeAsync) {
            return super::image_tasks::collect(&payload, transport, telemetry).await;
        }
        if matches!(kind, ImageResponseKind::Openai) {
            if payload.get("error").is_some_and(|error| !error.is_null()) {
                return Err(failure_from(&payload, status));
            }
            let data = payload.get("data").and_then(Value::as_array);
            if data.is_some_and(|data| !data.is_empty()) {
                if let Some(usage) = payload.get("usage") {
                    telemetry.report_usage(usage);
                }
                return Ok(payload);
            }
            return Err(failure_from(&payload, status));
        }
        gemini_frame(&payload, &mut images, &mut usage, &mut finished)?;
    }
    if images.is_empty() {
        return Err(ImageFailure::invalid(
            "上游没有返回生成图片（可能被内容策略拦截），未将文字回复当作生图成功",
        ));
    }
    let mut result = json!({ "created": crate::server::logging::now_ms() / 1000, "data": images });
    if let Some(usage) = usage {
        telemetry.report_usage(&usage);
        result["usage"] = usage;
    }
    Ok(result)
}

fn gemini_frame(
    frame: &Value,
    images: &mut Vec<Value>,
    usage: &mut Option<Value>,
    finished: &mut bool,
) -> Result<(), ImageFailure> {
    let response = frame.get("response").unwrap_or(frame);
    if response.get("error").is_some_and(|error| !error.is_null()) {
        return Err(failure_from(response, 200));
    }
    if frame.get("error").is_some_and(|error| !error.is_null()) {
        return Err(failure_from(frame, 200));
    }
    if let Some(metadata) = response
        .get("usageMetadata")
        .or_else(|| response.get("usage_metadata"))
    {
        let input = metadata
            .get("promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = metadata
            .get("candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let thought = metadata
            .get("thoughtsTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        *usage = Some(json!({
            "input_tokens": input,
            "output_tokens": output.saturating_add(thought),
            "total_tokens": metadata.get("totalTokenCount").and_then(Value::as_u64)
                .unwrap_or(input.saturating_add(output).saturating_add(thought)),
        }));
    }
    let Some(candidates) = response.get("candidates").and_then(Value::as_array) else {
        if response.pointer("/promptFeedback/blockReason").is_some() {
            return Err(ImageFailure {
                status: 400,
                body: json!({ "code": "content_policy_violation", "message": "上游内容策略拦截了生图提示词" }),
            });
        }
        return Ok(());
    };
    for candidate in candidates {
        if candidate.get("finishReason").and_then(Value::as_str) == Some("STOP") {
            *finished = true;
        }
        if let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        {
            for part in parts {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let Some(inline) = part.get("inlineData").or_else(|| part.get("inline_data"))
                else {
                    continue;
                };
                let Some(data) = inline
                    .get("data")
                    .and_then(Value::as_str)
                    .filter(|data| !data.is_empty())
                else {
                    continue;
                };
                let mime = inline
                    .get("mimeType")
                    .or_else(|| inline.get("mime_type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !mime.starts_with("image/") {
                    continue;
                }
                if !images
                    .iter()
                    .any(|image| image.get("b64_json").and_then(Value::as_str) == Some(data))
                {
                    images.push(json!({ "b64_json": data, "mime_type": mime }));
                }
            }
        }
        if candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .is_some_and(|reason| {
                matches!(reason, "SAFETY" | "IMAGE_SAFETY" | "PROHIBITED_CONTENT")
            })
        {
            return Err(ImageFailure {
                status: 400,
                body: json!({ "code": "content_policy_violation", "message": "上游内容策略拦截了图片输出" }),
            });
        }
    }
    Ok(())
}

fn failure_from(payload: &Value, http_status: u16) -> ImageFailure {
    let error = payload.get("error").unwrap_or(payload);
    let code = error.get("code").cloned().unwrap_or(Value::Null);
    let message = error
        .get("message")
        .or_else(|| error.get("msg"))
        .and_then(Value::as_str)
        .unwrap_or("上游没有返回有效的图片数据");
    let upstream_status = error.get("status").and_then(Value::as_str).unwrap_or("");
    let status = if http_status >= 400 {
        http_status
    } else {
        match upstream_status {
            "UNAUTHENTICATED" => 401,
            "PERMISSION_DENIED" => 403,
            "RESOURCE_EXHAUSTED" => 429,
            "INVALID_ARGUMENT" => 400,
            _ => code
                .as_u64()
                .filter(|code| (400..600).contains(code))
                .map(|code| code as u16)
                .unwrap_or(502),
        }
    };
    ImageFailure {
        status,
        body: json!({ "code": code, "message": format!("{upstream_status} {message}").trim() }),
    }
}

pub(super) fn output_error(message: impl Into<String>) -> GatewayError {
    GatewayError::with_status(502, message).with_code("image_generation_error")
}
