//! Names for stored templates, which also make their versions. An application
//! renders `invoice`; storing a new template and pointing `invoice` at it is a
//! new version, and every earlier one stays listed and renderable as
//! `invoice@2`. Nothing in the store is changed: each version is a record of its
//! own, and the newest is the alias. Aliases are scoped as templates are.

use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use rusticdoc_core::store::{self, AliasHistory, AliasMeta, AliasPage, MAX_ALIAS_VERSIONS};
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::routes::form::Form;
use crate::routes::templates::{ListParams, resolve_id, scope, store_of, user_store_error};
use crate::state::AppState;

/// The most aliases one page lists.
const MAX_PAGE: usize = 200;
const DEFAULT_PAGE: usize = 50;

/// What `PUT /v1/aliases/{name}` takes, as JSON or a multipart form.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct AliasSet {
    /// The template the alias names from now on: an id, or another alias.
    #[schema(
        rename = "templateId",
        example = "01966f3e-7c1a-7d3c-9a44-1d2f5a6b7c8d"
    )]
    pub template_id: String,
}

/// An alias and the template it names now.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct AliasSchema {
    #[schema(example = "invoice")]
    pub name: String,
    pub template_id: String,
    /// 1 for the first template it named, counted since it was last deleted.
    /// Render an earlier one as `name@version`.
    pub version: usize,
    /// RFC 3339: when it was pointed at this template.
    pub set_at: String,
}

/// One version of an alias.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct AliasVersionSchema {
    pub version: usize,
    pub template_id: String,
    /// RFC 3339.
    pub set_at: String,
}

/// An alias, the template it names now, and every version, oldest first.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct AliasHistorySchema {
    pub name: String,
    pub template_id: String,
    pub version: usize,
    pub set_at: String,
    pub versions: Vec<AliasVersionSchema>,
}

/// One page of aliases.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct AliasList {
    pub aliases: Vec<AliasSchema>,
    /// Pass as `cursor` for the next page. Absent on the last.
    pub next_cursor: Option<String>,
}

fn bad_name(name: &str) -> ApiError {
    let shown: String = name.chars().take(64).filter(|c| !c.is_control()).collect();
    ApiError::bad_request(format!(
        "'{shown}' cannot be an alias: use 1 to 64 letters, digits, '.', '_' and '-', starting with a letter or digit, and not shaped like a template id"
    ))
}

/// Set an alias
///
/// Points an alias at a template. A new alias starts at version 1; pointing an
/// existing one elsewhere adds a version, and the earlier ones stay. Every
/// replica sees the change at once.
#[utoipa::path(
    put, path = "/v1/aliases/{name}", tag = "templates", operation_id = "setAlias",
    params(("name" = String, Path, description = "The alias, such as `invoice`")),
    request_body(content(
        (AliasSet = "application/json"),
        (AliasSet = "multipart/form-data"),
    )),
    responses(
        (status = 200, description = "A new version of an alias", body = AliasSchema),
        (status = 201, description = "A new alias", body = AliasSchema),
        (status = 400, description = "Not a usable name, or no template named", body = ErrorBody),
        (status = 403, description = "Too many aliases, or versions of this one", body = ErrorBody),
        (status = 404, description = "This caller has no such template", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn set(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(name): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let store = store_of(&state)?;
    if !store::is_alias_name(&name) {
        return Err(bad_name(&name));
    }
    let form = Form::from_request(req, &state).await?;
    let reference = form
        .field("templateId")
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .ok_or_else(|| ApiError::bad_request("send the 'templateId' the alias is to name"))?
        .to_string();
    let scope = scope(&state, &principal);
    let id = resolve_id(&store, &scope, &reference).await?;
    let versions = store.alias_versions(&scope, &name).await?;
    if versions >= MAX_ALIAS_VERSIONS {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "quota_exceeded",
            format!(
                "'{name}' has {versions} versions, the most an alias keeps; delete it to start again"
            ),
        ));
    }
    if versions == 0 {
        let count = store.alias_count(&scope).await?;
        if count >= state.config.max_templates {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "quota_exceeded",
                format!(
                    "this caller holds {count} aliases; the limit is {}, as for templates",
                    state.config.max_templates
                ),
            ));
        }
    }
    let meta: AliasMeta = store
        .set_alias(&scope, &name, &id)
        .await
        .map_err(user_store_error)?;
    tracing::info!(
        log_type = "audit", action = "alias.set", caller = %principal.short(),
        alias = %name, template = %id, version = meta.version, "alias set"
    );
    let root = state.config.root_path.trim_end_matches('/');
    let status = if meta.version == 1 {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        [(header::LOCATION, format!("{root}/v1/aliases/{name}"))],
        Json(meta),
    )
        .into_response())
}

/// Get an alias
///
/// An alias, the template it names now, and its history: every version since
/// it was last deleted, oldest first.
#[utoipa::path(
    get, path = "/v1/aliases/{name}", tag = "templates", operation_id = "getAlias",
    params(("name" = String, Path, description = "The alias")),
    responses(
        (status = 200, description = "The alias and its versions", body = AliasHistorySchema),
        (status = 404, description = "This caller has no such alias", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn show(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(name): Path<String>,
) -> ApiResult<Json<AliasHistory>> {
    let store = store_of(&state)?;
    let history = store
        .alias_history(&scope(&state, &principal), &name)
        .await
        .map_err(user_store_error)?;
    Ok(Json(history))
}

/// List aliases
///
/// Lists this caller's aliases by name, each with the template it names now.
#[utoipa::path(
    get, path = "/v1/aliases", tag = "templates", operation_id = "listAliases",
    params(ListParams),
    responses(
        (status = 200, description = "A page of aliases", body = AliasList),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<AliasPage>> {
    let store = store_of(&state)?;
    let limit = params.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let page = store
        .list_aliases(&scope(&state, &principal), limit, params.cursor.as_deref())
        .await?;
    Ok(Json(page))
}

/// Delete an alias
///
/// Deletes an alias and its history. The templates it named stay. Setting it
/// again starts at version 1.
#[utoipa::path(
    delete, path = "/v1/aliases/{name}", tag = "templates", operation_id = "deleteAlias",
    params(("name" = String, Path, description = "The alias")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "This caller has no such alias", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn remove(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let store = store_of(&state)?;
    store
        .delete_alias(&scope(&state, &principal), &name)
        .await
        .map_err(user_store_error)?;
    tracing::info!(
        log_type = "audit", action = "alias.delete", caller = %principal.short(),
        alias = %name, "alias deleted"
    );
    Ok(StatusCode::NO_CONTENT)
}
