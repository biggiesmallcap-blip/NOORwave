//! Which Explore scenes the listener wants as stations. A scene that is
//! switched off is never built, so it costs nothing and never shows.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::Scene;

const KEY: &str = "video_stations.explore";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExploreSettings {
    /// Master switch for the whole Explore row.
    pub enabled: bool,
    /// Scene slugs the listener switched off.
    pub hidden: Vec<String>,
}

impl Default for ExploreSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            hidden: Vec::new(),
        }
    }
}

impl ExploreSettings {
    /// Known slugs only, each once, in scene order.
    pub fn normalized(self) -> Self {
        let hidden = Scene::ALL
            .iter()
            .map(|scene| scene.slug())
            .filter(|slug| self.hidden.iter().any(|h| h == slug))
            .map(str::to_string)
            .collect();
        Self {
            enabled: self.enabled,
            hidden,
        }
    }

    /// Scenes to build, in display order.
    pub fn scenes(&self) -> Vec<Scene> {
        if !self.enabled {
            return Vec::new();
        }
        Scene::ALL
            .into_iter()
            .filter(|scene| !self.hidden.iter().any(|h| h == scene.slug()))
            .collect()
    }
}

pub fn load(conn: &Connection) -> Result<ExploreSettings> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw
        .and_then(|raw| serde_json::from_str::<ExploreSettings>(&raw).ok())
        .unwrap_or_default()
        .normalized())
}

pub fn save(conn: &Connection, settings: &ExploreSettings) -> Result<ExploreSettings> {
    let settings = settings.clone().normalized();
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![KEY, serde_json::to_string(&settings)?],
    )?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_stations::pool::tests::conn;

    #[test]
    fn every_scene_is_on_until_switched_off() {
        let conn = conn();
        assert_eq!(load(&conn).unwrap().scenes(), Scene::ALL.to_vec());

        let saved = save(
            &conn,
            &ExploreSettings {
                enabled: true,
                hidden: vec![
                    "k-pop".into(),
                    "nonsense".into(),
                    "latin".into(),
                    "k-pop".into(),
                ],
            },
        )
        .unwrap();
        assert_eq!(saved.hidden, vec!["latin".to_string(), "k-pop".to_string()]);
        let scenes = load(&conn).unwrap().scenes();
        assert!(!scenes.contains(&Scene::Latin));
        assert!(!scenes.contains(&Scene::Kpop));
        assert!(scenes.contains(&Scene::Reggaeton));

        save(
            &conn,
            &ExploreSettings {
                enabled: false,
                hidden: Vec::new(),
            },
        )
        .unwrap();
        assert!(load(&conn).unwrap().scenes().is_empty());
    }
}
