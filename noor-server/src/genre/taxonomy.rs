use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;

const TAXONOMY_JSON: &str = include_str!("../../../genre-taxonomy/taxonomy.json");

#[derive(Debug, Deserialize)]
struct TaxonomyNode {
    name: String,
    #[serde(default)]
    children: Vec<TaxonomyNode>,
}

pub fn ensure_taxonomy_loaded(conn: &Connection) -> Result<usize> {
    let taxonomy: TaxonomyNode = serde_json::from_str(TAXONOMY_JSON)?;
    let existing_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM genres", [], |row| row.get(0))?;
    if existing_count > 0 {
        return Ok(existing_count as usize);
    }

    let tx = conn.unchecked_transaction()?;
    let mut inserted = 0usize;
    for child in taxonomy.children {
        insert_node(&tx, &child, None, &mut inserted)?;
    }
    tx.commit()?;
    Ok(inserted)
}

fn insert_node(
    conn: &Connection,
    node: &TaxonomyNode,
    parent_id: Option<i64>,
    inserted: &mut usize,
) -> Result<i64> {
    let slug = slugify(&node.name);
    conn.execute(
        "INSERT OR IGNORE INTO genres (name, slug, parent_id) VALUES (?1, ?2, ?3)",
        params![node.name, slug, parent_id],
    )?;
    *inserted += 1;

    let id = conn.last_insert_rowid();
    for child in &node.children {
        insert_node(conn, child, Some(id), inserted)?;
    }

    Ok(id)
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn slugify_normalizes_names() {
        assert_eq!(slugify("Drum and Bass"), "drum-and-bass");
        assert_eq!(slugify("Post-Punk"), "post-punk");
        assert_eq!(slugify("Hi-NRG"), "hi-nrg");
    }
}
