//! The HTTP command surface: `POST /api/<command>` with exactly the arguments
//! `invoke()` sends and the same JSON shapes back (PLAN.md §12.3).
//!
//! One dispatch table maps the frozen command names (`PLAN.md` §12.2) onto the
//! `&AppState` functions the Tauri wrappers call, so the two shells can never
//! drift.

use app_lib::commands::{
    chunks, endpoint, export, glossary, ingest, jobs, metrics, misc, project, recon, review,
    role_binding, series, sidecar, translation,
};
use app_lib::error::AppError;
use app_lib::AppState;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

/// The argument named `key` of the request body, deserialized.
///
/// A missing key becomes `null` so an `Option` argument deserializes to `None` and a
/// required one fails with its own serde message.
fn arg<T: DeserializeOwned>(body: &Value, key: &str) -> Result<T, AppError> {
    let value = body.get(key).cloned().unwrap_or(Value::Null);
    serde_json::from_value(value)
        .map_err(|error| AppError::Invalid(format!("invalid argument '{key}': {error}")))
}

/// Serialize a command result, the way `invoke()` resolves it.
fn json<T: Serialize>(result: Result<T, AppError>) -> Result<Value, AppError> {
    result.and_then(|value| serde_json::to_value(value).map_err(AppError::from))
}

/// The HTTP status a domain error maps to. The body is always the serialized [`AppError`],
/// the same `{code, message, retryable}` object `invoke()` rejects with.
pub fn status_for(error: &AppError) -> StatusCode {
    match error {
        AppError::NotFound(_) => StatusCode::NOT_FOUND,
        AppError::Invalid(_) | AppError::Json(_) => StatusCode::BAD_REQUEST,
        AppError::Endpoint { .. } => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Run one command. `body` is the argument object the UI would pass to `invoke`.
pub async fn dispatch(state: &AppState, command: &str, body: &Value) -> Result<Value, AppError> {
    match command {
        // --- projects -----------------------------------------------------
        "project_list" => json(project::project_list(state).await),
        "project_create" => json(project::project_create(state, arg(body, "req")?).await),
        "project_get" => json(project::project_get(state, arg(body, "id")?).await),
        "project_delete" => json(project::project_delete(state, arg(body, "id")?).await),
        "project_export" => json(project::project_export(state, arg(body, "req")?).await),
        "project_import" => json(project::project_import(state, arg(body, "req")?).await),

        // --- endpoints and role bindings ----------------------------------
        "endpoint_list" => json(endpoint::endpoint_list(state).await),
        "endpoint_upsert" => json(endpoint::endpoint_upsert(state, arg(body, "req")?).await),
        "endpoint_delete" => json(endpoint::endpoint_delete(state, arg(body, "id")?).await),
        "endpoint_test" => json(endpoint::endpoint_test(state, arg(body, "id")?).await),
        "endpoint_models" => json(endpoint::endpoint_models(state, arg(body, "req")?).await),
        "role_binding_list" => json(role_binding::role_binding_list(state).await),
        "role_binding_set" => json(role_binding::role_binding_set(state, arg(body, "req")?).await),
        "role_binding_delete" => {
            json(role_binding::role_binding_delete(state, arg(body, "id")?).await)
        }

        // --- ingestion and translation ------------------------------------
        "document_inspect" => json(ingest::document_inspect(state, arg(body, "path")?).await),
        "ingest_start" => json(ingest::ingest_start(state, arg(body, "req")?).await),
        "translation_start" => json(translation::translation_start(state, arg(body, "req")?).await),
        "translation_pause" => json(translation::translation_pause(state).await),
        "translation_cancel" => {
            json(translation::translation_cancel(state, arg(body, "req")?).await)
        }

        // --- book reconnaissance ------------------------------------------
        "recon_start" => json(recon::recon_start(state, arg(body, "req")?).await),
        "recon_get" => json(recon::recon_get(state, arg(body, "project_id")?).await),
        "recon_confirm" => json(recon::recon_confirm(state, arg(body, "req")?).await),
        "project_set_dialogue_style" => {
            json(recon::project_set_dialogue_style(state, arg(body, "req")?).await)
        }

        // --- glossary ------------------------------------------------------
        "glossary_list" => json(glossary::glossary_list(state, arg(body, "project_id")?).await),
        "glossary_upsert" => json(glossary::glossary_upsert(state, arg(body, "req")?).await),
        "glossary_delete" => json(glossary::glossary_delete(state, arg(body, "id")?).await),

        // --- series ---------------------------------------------------------
        "series_list" => json(series::series_list(state).await),
        "series_create" => json(series::series_create(state, arg(body, "req")?).await),
        "series_get" => json(series::series_get(state, arg(body, "id")?).await),
        "series_update" => json(series::series_update(state, arg(body, "req")?).await),
        "series_delete" => json(series::series_delete(state, arg(body, "id")?).await),
        "project_set_series" => json(series::project_set_series(state, arg(body, "req")?).await),
        "series_glossary_list" => {
            json(series::series_glossary_list(state, arg(body, "series_id")?).await)
        }
        "series_glossary_upsert" => {
            json(series::series_glossary_upsert(state, arg(body, "req")?).await)
        }
        "series_glossary_delete" => {
            json(series::series_glossary_delete(state, arg(body, "id")?).await)
        }
        "series_variant_upsert" => {
            json(series::series_variant_upsert(state, arg(body, "req")?).await)
        }
        "series_variant_delete" => {
            json(series::series_variant_delete(state, arg(body, "id")?).await)
        }
        "series_promote_term" => json(series::series_promote_term(state, arg(body, "req")?).await),
        "series_export" => json(series::series_export(state, arg(body, "req")?).await),
        "series_import" => json(series::series_import(state, arg(body, "req")?).await),
        "series_qa_scan" => json(series::series_qa_scan(state, arg(body, "series_id")?).await),
        "series_recon_start" => json(series::series_recon_start(state, arg(body, "req")?).await),
        "series_recon_confirm" => {
            json(series::series_recon_confirm(state, arg(body, "req")?).await)
        }

        // --- review and QA ---------------------------------------------------
        "review_start" => json(review::review_start(state, arg(body, "req")?).await),
        "suggestion_list" => json(review::suggestion_list(state, arg(body, "req")?).await),
        "suggestion_history" => json(review::suggestion_history(state, arg(body, "req")?).await),
        "suggestion_accept" => json(review::suggestion_accept(state, arg(body, "id")?).await),
        "suggestion_reject" => json(review::suggestion_reject(state, arg(body, "id")?).await),
        "qa_report" => json(review::qa_report(state, arg(body, "req")?).await),
        "qa_finding_set_status" => {
            json(review::qa_finding_set_status(state, arg(body, "req")?).await)
        }

        // --- queue and chunks -------------------------------------------------
        "job_list" => json(jobs::job_list(state, arg(body, "req")?).await),
        "job_cancel" => json(jobs::job_cancel(state, arg(body, "req")?).await),
        "chunk_list" => json(chunks::chunk_list(state, arg(body, "req")?).await),
        "chunk_get" => json(chunks::chunk_get(state, arg(body, "chunk_id")?).await),

        // --- runtime ------------------------------------------------------------
        "metrics_get" => json(metrics::metrics_get(state).await),
        "sidecar_status" => json(sidecar::sidecar_status(state).await),
        "export_build" => json(export::export_build(state, arg(body, "req")?).await),
        "export_preview" => json(export::export_preview(state, arg(body, "req")?).await),
        "export_history" => json(export::export_history(state, arg(body, "project_id")?).await),
        "log_frontend_error" => {
            json(misc::log_frontend_error(arg(body, "command")?, arg(body, "message")?).await)
        }
        "diagnostics_paths" => json(misc::diagnostics_paths(state).await),
        "diagnostics_export" => json(misc::diagnostics_export(state).await),

        // Opening a path in the OS needs the desktop shell; a browser downloads instead
        // (PLAN.md §12.3).
        "open_path" => Err(AppError::Invalid(
            "open_path is only available in the desktop app".into(),
        )),

        other => Err(AppError::Invalid(format!("unknown command '{other}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::{arg, status_for};
    use app_lib::error::AppError;
    use axum::http::StatusCode;
    use serde_json::json;

    #[test]
    fn a_missing_argument_becomes_null() {
        let value: Option<String> = arg(&json!({}), "req").expect("absent optional argument");
        assert!(value.is_none());
        // A required one fails naming the argument.
        let error = arg::<String>(&json!({}), "id").expect_err("required argument");
        assert!(error.to_string().contains("invalid argument 'id'"));
    }

    #[test]
    fn a_wrong_argument_shape_is_invalid_not_internal() {
        let error = arg::<i64>(&json!({ "limit": "many" }), "limit").expect_err("wrong type");
        assert_eq!(status_for(&error), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn domain_errors_map_to_their_http_status() {
        assert_eq!(
            status_for(&AppError::NotFound("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(&AppError::Invalid("x".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(&AppError::Other(anyhow::anyhow!("boom"))),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
