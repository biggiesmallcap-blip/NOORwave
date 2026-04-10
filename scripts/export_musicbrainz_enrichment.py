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
        SELECT t.tidal_id, g.slug, tg.confidence
        FROM track_genres tg
        JOIN tracks t ON t.id = tg.track_id
        JOIN genres g ON g.id = tg.genre_id
        WHERE tg.source = 'musicbrainz'
          AND t.tidal_id IS NOT NULL
        ORDER BY t.tidal_id, g.slug
        """
    )
    count = 0
    with destination.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(["tidal_id", "genre_slug", "confidence"])
        for tidal_id, genre_slug, confidence in rows:
            writer.writerow([tidal_id, genre_slug, confidence])
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
    genres_path = out_dir / "musicbrainz_genres.csv"
    manifest_path = out_dir / "manifest.json"

    conn = sqlite3.connect(db_path)
    try:
        checked_count = export_checked(conn, checked_path)
        genre_count = export_genres(conn, genres_path)
    finally:
        conn.close()

    manifest = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "db_path": str(db_path),
        "checked_rows": checked_count,
        "genre_rows": genre_count,
        "files": {
            "checked": checked_path.name,
            "genres": genres_path.name,
        },
    }
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    print(f"Exported {checked_count} checked tracks to {checked_path}")
    print(f"Exported {genre_count} genre rows to {genres_path}")
    print(f"Wrote manifest to {manifest_path}")


if __name__ == "__main__":
    main()
