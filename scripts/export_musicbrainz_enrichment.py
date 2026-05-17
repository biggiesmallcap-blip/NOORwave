#!/usr/bin/env python3

from __future__ import annotations

import argparse
import csv
import json
import sqlite3
from datetime import datetime, timezone
from pathlib import Path


def export_checked(conn: sqlite3.Connection, destination: Path) -> int:
    rows = conn.execute(
        """
        SELECT t.tidal_id
        FROM musicbrainz_checked mc
        JOIN tracks t ON t.id = mc.track_id
        WHERE t.tidal_id IS NOT NULL
        ORDER BY t.tidal_id
        """
    )
    count = 0
    with destination.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(["tidal_id"])
        for (tidal_id,) in rows:
            writer.writerow([tidal_id])
            count += 1
    return count


def export_genres(conn: sqlite3.Connection, destination: Path) -> int:
    rows = conn.execute(
        """
        SELECT t.tidal_id, tg.source, g.slug, tg.confidence
        FROM track_genres tg
        JOIN tracks t ON t.id = tg.track_id
        JOIN genres g ON g.id = tg.genre_id
        WHERE tg.source IN ('musicbrainz', 'lastfm')
          AND t.tidal_id IS NOT NULL
        ORDER BY t.tidal_id, tg.source, g.slug
        """
    )
    count = 0
    with destination.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(["tidal_id", "source", "genre_slug", "confidence"])
        for tidal_id, source, genre_slug, confidence in rows:
            writer.writerow([tidal_id, source, genre_slug, confidence])
            count += 1
    return count


def export_lastfm_checked(conn: sqlite3.Connection, destination: Path) -> int:
    rows = conn.execute(
        """
        SELECT t.tidal_id
        FROM lastfm_checked lc
        JOIN tracks t ON t.id = lc.track_id
        WHERE t.tidal_id IS NOT NULL
        ORDER BY t.tidal_id
        """
    )
    count = 0
    with destination.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(["tidal_id"])
        for (tidal_id,) in rows:
            writer.writerow([tidal_id])
            count += 1
    return count


def export_context_tags(conn: sqlite3.Connection, destination: Path) -> int:
    rows = conn.execute(
        """
        SELECT t.tidal_id, tct.tag, tct.normalized_tag, tct.context, tct.confidence
        FROM track_context_tags tct
        JOIN tracks t ON t.id = tct.track_id
        WHERE tct.source = 'lastfm'
          AND t.tidal_id IS NOT NULL
        ORDER BY t.tidal_id, tct.context, tct.normalized_tag
        """
    )
    count = 0
    with destination.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(["tidal_id", "tag", "normalized_tag", "context", "confidence"])
        for tidal_id, tag, normalized_tag, context, confidence in rows:
            writer.writerow([tidal_id, tag, normalized_tag, context, confidence])
            count += 1
    return count


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Export portable MusicBrainz enrichment data from a NOOR SQLite database."
    )
    parser.add_argument("--db", default="noor.db", help="Path to the NOOR SQLite database.")
    parser.add_argument(
        "--out-dir",
        default="data/musicbrainz",
        help="Directory where the export files will be written.",
    )
    args = parser.parse_args()

    db_path = Path(args.db)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    checked_path = out_dir / "musicbrainz_checked.csv"
    lastfm_checked_path = out_dir / "lastfm_checked.csv"
    genres_path = out_dir / "musicbrainz_genres.csv"
    context_tags_path = out_dir / "lastfm_context_tags.csv"
    manifest_path = out_dir / "manifest.json"

    conn = sqlite3.connect(db_path)
    try:
        checked_count = export_checked(conn, checked_path)
        lastfm_checked_count = export_lastfm_checked(conn, lastfm_checked_path)
        genre_count = export_genres(conn, genres_path)
        context_tag_count = export_context_tags(conn, context_tags_path)
    finally:
        conn.close()

    manifest = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "db_path": str(db_path),
        "checked_rows": checked_count,
        "genre_rows": genre_count,
        "lastfm_checked_rows": lastfm_checked_count,
        "context_tag_rows": context_tag_count,
        "files": {
            "checked": checked_path.name,
            "genres": genres_path.name,
            "lastfm_checked": lastfm_checked_path.name,
            "context_tags": context_tags_path.name,
        },
    }
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    print(f"Exported {checked_count} checked tracks to {checked_path}")
    print(f"Exported {lastfm_checked_count} Last.fm checked tracks to {lastfm_checked_path}")
    print(f"Exported {genre_count} genre rows to {genres_path}")
    print(f"Exported {context_tag_count} context tag rows to {context_tags_path}")
    print(f"Wrote manifest to {manifest_path}")


if __name__ == "__main__":
    main()
