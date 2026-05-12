use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use loco_rs::app::AppContext;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};

use crate::models::{
    api_token::{ApiTokenEntity, Column as ApiTokenColumn},
    post::{ActiveModel, Column, Model, PostEntity},
};

#[derive(Deserialize)]
pub struct CreatePost {
    pub title: String,
    pub body: String,
    pub published: Option<bool>,
    pub author: String,
}

#[derive(Deserialize)]
pub struct UpdatePost {
    pub title: Option<String>,
    pub body: Option<String>,
    pub published: Option<bool>,
    pub author: Option<String>,
}

#[derive(Serialize)]
struct ErrBody { error: String }

fn validation_error(msg: &str) -> (StatusCode, Json<ErrBody>) {
    (StatusCode::UNPROCESSABLE_ENTITY, Json(ErrBody { error: msg.into() }))
}

fn validate_create(p: &CreatePost) -> Result<(), &'static str> {
    if p.title.trim().is_empty() { return Err("title must not be blank"); }
    if p.title.len() > 255 { return Err("title must be 255 characters or fewer"); }
    if p.body.trim().is_empty() { return Err("body must not be blank"); }
    if p.author.trim().is_empty() { return Err("author must not be blank"); }
    Ok(())
}

pub async fn list(State(ctx): State<AppContext>) -> impl IntoResponse {
    let posts = PostEntity::find()
        .order_by_desc(Column::CreatedAt)
        .limit(50)
        .all(&ctx.db)
        .await
        .unwrap_or_default();
    Json(posts)
}

pub async fn show(
    State(ctx): State<AppContext>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match PostEntity::find_by_id(id).one(&ctx.db).await {
        Ok(Some(p)) => (StatusCode::OK, Json(serde_json::to_value(p).unwrap())).into_response(),
        _           => (StatusCode::NOT_FOUND, Json(ErrBody { error: "not found".into() })).into_response(),
    }
}

pub async fn create(
    State(ctx): State<AppContext>,
    Json(body): Json<CreatePost>,
) -> impl IntoResponse {
    if let Err(msg) = validate_create(&body) {
        return (StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": msg}))).into_response();
    }
    let model = ActiveModel {
        title:     ActiveValue::Set(body.title),
        body:      ActiveValue::Set(body.body),
        published: ActiveValue::Set(body.published.unwrap_or(false)),
        author:    ActiveValue::Set(body.author),
        ..Default::default()
    };
    match model.insert(&ctx.db).await {
        Ok(post) => (StatusCode::CREATED, Json(post)).into_response(),
        Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR,
                     Json(ErrBody { error: e.to_string() })).into_response(),
    }
}

pub async fn update(
    State(ctx): State<AppContext>,
    Path(id): Path<i64>,
    Json(body): Json<UpdatePost>,
) -> impl IntoResponse {
    match PostEntity::find_by_id(id).one(&ctx.db).await {
        Ok(Some(existing)) => {
            let mut am: ActiveModel = existing.into();
            if let Some(t) = body.title     { am.title     = ActiveValue::Set(t); }
            if let Some(b) = body.body      { am.body      = ActiveValue::Set(b); }
            if let Some(p) = body.published { am.published = ActiveValue::Set(p); }
            if let Some(a) = body.author    { am.author    = ActiveValue::Set(a); }
            match am.update(&ctx.db).await {
                Ok(post) => Json(post).into_response(),
                Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR,
                             Json(ErrBody { error: e.to_string() })).into_response(),
            }
        }
        _ => (StatusCode::NOT_FOUND, Json(ErrBody { error: "not found".into() })).into_response(),
    }
}

pub async fn delete(
    State(ctx): State<AppContext>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match PostEntity::find_by_id(id).one(&ctx.db).await {
        Ok(Some(p)) => {
            let am: ActiveModel = p.into();
            let _ = am.delete(&ctx.db).await;
            StatusCode::NO_CONTENT.into_response()
        }
        _ => (StatusCode::NOT_FOUND, Json(ErrBody { error: "not found".into() })).into_response(),
    }
}

pub async fn protected(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let raw = match headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(t) => t.to_owned(),
        None => return (StatusCode::UNAUTHORIZED,
                        Json(ErrBody { error: "missing or invalid Authorization header".into() })).into_response(),
    };

    let token_row = ApiTokenEntity::find()
        .filter(ApiTokenColumn::Token.eq(&raw))
        .one(&ctx.db)
        .await;

    match token_row {
        Ok(Some(t)) => {
            let total: u64 = PostEntity::find().count(&ctx.db).await.unwrap_or(0);
            Json(serde_json::json!({ "principal": t.principal, "total_posts": total })).into_response()
        }
        _ => (StatusCode::UNAUTHORIZED, Json(ErrBody { error: "invalid token".into() })).into_response(),
    }
}

// Server-rendered HTML (minimal, no templating engine dependency)
pub async fn html_list(State(ctx): State<AppContext>) -> impl IntoResponse {
    let posts: Vec<Model> = PostEntity::find()
        .order_by_desc(Column::CreatedAt)
        .limit(50)
        .all(&ctx.db)
        .await
        .unwrap_or_default();

    let rows: String = posts.iter().map(|p| {
        format!(
            r#"<li><a href="/posts/{}">{}</a> &mdash; {}{}</li>"#,
            p.id, p.title, p.author,
            if !p.published { " <em>[draft]</em>" } else { "" }
        )
    }).collect();

    let body = format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Posts</title></head>\
         <body><h1>Posts</h1><ul>{}</ul></body></html>",
        rows
    );

    axum::response::Html(body)
}

pub async fn html_show(
    State(ctx): State<AppContext>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match PostEntity::find_by_id(id).one(&ctx.db).await {
        Ok(Some(p)) => {
            let draft = if !p.published { "<em>Draft</em>" } else { "" };
            let body = format!(
                "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{}</title></head>\
                 <body><h1>{}</h1><p>By {}</p>{}<div>{}</div></body></html>",
                p.title, p.title, p.author, draft, p.body
            );
            axum::response::Html(body).into_response()
        }
        _ => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
