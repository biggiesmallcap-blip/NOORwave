use super::*;

pub(super) async fn status(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    s.db.with_conn(|conn| {
        let mut stmt=conn.prepare("SELECT t.id,a.availability,t.remote_favorite_state,
            (SELECT COUNT(*) FROM tidal_track_aliases alt WHERE alt.track_id=t.id)
            FROM tracks t JOIN tidal_track_aliases a ON a.tidal_id=t.tidal_id
            WHERE (t.is_library=1 OR t.is_favorite=1)
            AND (a.availability='unavailable' OR (t.remote_favorite_state='unresolved'
                AND (t.is_favorite=1 OR EXISTS(SELECT 1 FROM tidal_track_aliases saved WHERE saved.track_id=t.id AND saved.favorite_created IS NOT NULL))))")?;
        let rows=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,
            "availability":r.get::<_,String>(1)?,"favorite_state":r.get::<_,String>(2)?,"releases":r.get::<_,i64>(3)?})))?
            .collect::<Result<Vec<_>,_>>()?;
        Ok(Json(json!({"tracks":rows})))
    }).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)
}
