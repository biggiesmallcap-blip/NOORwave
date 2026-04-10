#!/usr/bin/env python3

from __future__ import annotations

import argparse
import csv
import sqlite3
from pathlib import Path


def load_track_map(conn: sqlite3.Connection) -> dict[int, int]:
    rows = conn.execute("SELECT tidal_id, id FROM tracks WHERE tidal_id IS NOT NULL")
    return {int(tidal_id): int(track_id) for tidal_id, track_id in rows}


def load_genre_map(conn: sqlite3.Connection) -> dict[str, int]:
    rows = conn.execute("SELECT slug, id FROM genres")
    return {str(slug): int(genre_id) for slug, genre_id in rows}


def import_checked(
    conn: sqlite3.Connection, checked_path: Path, track_map: dict[int, int]
) -> tuple[int, int]:
    inserted = 0
    skipped = 0
    with checked_path.open("r", newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        for row in reader:
            tidal_id = int(row["tidal_id"])
            track_id = track_map.get(tidal_id)
            if track_id is None:
                skipped += 1
                continue
            before = conn.total_changes
            conn.execute(
                "INSERT OR IGNORE INTO musicbrainz_checked (track_id) VALUES (?)",
                (track_id,),
            )
            inserted += int(conn.total_changes > before)
    return inserted, skipped


def import_genres(
    conn: sqlite3.Connection,
    genres_path: Path,
    track_map: dict[int, int],
    genre_map: dict[str, int],
) -> tuple[int, int, int]:
    inserted = 0
    skipped_tracks = 0
    skipped_genres = 0
    with genres_path.open("r", newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        for row in reader:
            tidal_id = int(row["tidal_id"])
            genre_slug = row["genre_slug"]
            confidence = float(row["confidence"])

            track_id = track_map.get(tidal_id)
            if track_id is None:
                skipped_tracks += 1
                continue

            genre_id = genre_map.get(genre_slug)
            if genre_id is None:
                skipped_genres += 1
                continue

            before = conn.total_changes
            conn.execute(
                """
                INSERT OR IGNORE INTO track_genres (track_id, genre_id, source, confidence)
                VALUES (?, ?, 'musicbrainz', ?)
                """,
                (track_id, genre_id, confidence),
            )
            inserted += int(conn.total_changes > before)
    return inserted, skipped_tracks, skipped_genres


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Import portable MusicBrainz enrichment data into a NOOR SQLite database."
    )
    parser.add_argument("--db", default="noor.db", help="Path to the NOOR SQLite database.")
    parser.add_argument(
        "--from-dir",
        default="data/musicbrainz",
        help="Directory containing musicbrainz_checked.csv and musicbrainz_genres.csv.",
    )
    args = parser.parse_args()

    db_path = Path(args.db)
    source_dir = Path(args.from_dir)
    checked_path = source_dir / "musicbrainz_checked.csv"
    genres_path = source_dir / "musicbrainz_genres.csv"

    if not checked_path.exists():
        raise SystemExit(f"Missing checked export: {checked_path}")
    if not genres_path.exists():
        raise SystemExit(f"Missing genre export: {genres_path}")

    conn = sqlite3.connect(db_path)
    try:
        track_map = load_track_map(conn)
        genre_map = load_genre_map(conn)
        with conn:
            checked_inserted, checked_skipped = import_checked(conn, checked_path, track_map)
            genre_inserted, track_skipped, genre_skipped = import_genres(
                conn, genres_path, track_map, genre_map
            )
    finally:
        conn.close()

    print(f"Inserted {checked_inserted} checked markers; skipped {checked_skipped} missing tracks")
    print(
        "Inserted "
        f"{genre_inserted} genre rows; skipped {track_skipped} missing tracks and "
        f"{genre_skipped} missing genres"
    )


if __name__ == "__main__":
    main()
