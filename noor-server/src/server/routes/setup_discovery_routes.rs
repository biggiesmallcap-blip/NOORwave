use crate::{SharedState, db::discovery_setup};
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

async fn account(state: &SharedState) -> Result<Option<String>, StatusCode> {
    let tokens = state.read().await.tidal.tokens();
    let tokens = match tokens {
        Some(tokens) => Some(tokens),
        None => super::load_persisted_tidal_tokens(state)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    };
    Ok(tokens.map(|tokens| tokens.user_id.to_string()))
}

pub(super) async fn get_status(
    State(state): State<SharedState>,
) -> Result<Json<discovery_setup::Status>, StatusCode> {
    let account = account(&state).await?;
    let status = state
        .read()
        .await
        .db
        .with_conn(|conn| discovery_setup::status(conn, account.as_deref()))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(status))
}

#[derive(Deserialize)]
pub(super) struct Action {
    account_id: String,
    owner: String,
    action: String,
}

pub(super) async fn update(
    State(state): State<SharedState>,
    Json(action): Json<Action>,
) -> Result<Json<Value>, StatusCode> {
    if action.owner.is_empty() || action.owner.len() > 100 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let account = account(&state).await?.ok_or(StatusCode::CONFLICT)?;
    // A stale tab must not consume guidance for an account connected later.
    if account != action.account_id {
        return Err(StatusCode::CONFLICT);
    }
    if !["reserve", "shown", "release"].contains(&action.action.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let accepted = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            let now = chrono::Utc::now().timestamp();
            match action.action.as_str() {
                "reserve" => discovery_setup::reserve(conn, &account, &action.owner, now),
                "shown" => discovery_setup::acknowledge(conn, &account, &action.owner, now),
                _ => {
                    discovery_setup::release(conn, &account, &action.owner)?;
                    Ok(true)
                }
            }
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "accepted": accepted })))
}
