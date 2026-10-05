"""Audit historical catalogue changes; apply an explicit manifest locally.

Uses only SQLite. Never contacts TIDAL or changes favorite flags. Apply requires
the new application migration, verifies the manifest against current rows,
creates a consistent SQLite backup, and records every repair transactionally.
"""
import argparse
import datetime as dt
import json
import pathlib
import re
import sqlite3
from contextlib import closing


def read_only(path):
    path = pathlib.Path(path).resolve()
    # Keep SQLite's normal snapshot/locking behavior, including committed WAL
    # data. A momentarily absent WAL does not prove a live file is immutable.
    conn = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA query_only=ON")
    return conn


def timestamp(value):
    if not value:
        return None
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
        return (parsed if parsed.tzinfo else parsed.replace(tzinfo=dt.timezone.utc)).astimezone(dt.timezone.utc)
    except ValueError:
        return None


def normalize(value):
    return re.sub(r"\W+", " ", value.casefold()).strip()


def rows(conn):
    return [dict(row) for row in conn.execute("""
        SELECT t.id,t.tidal_id,t.title,t.isrc,t.duration_ms,t.date_added,
        t.is_favorite,a.name artist,al.title album,al.tidal_id album_tidal_id
        FROM tracks t JOIN artists a ON a.id=t.artist_id
        LEFT JOIN albums al ON al.id=t.album_id WHERE t.tidal_id>0
    """)]


def same_recording(before, after):
    duration_a, duration_b = before["duration_ms"] or 0, after["duration_ms"] or 0
    gap = abs(duration_a - duration_b)
    return (normalize(before["title"]) == normalize(after["title"])
            and normalize(before["artist"]) == normalize(after["artist"])
            and duration_a > 0 and duration_b > 0
            and gap <= 15000 and gap * 100 <= max(duration_a, duration_b) * 8)


def audit(database, sources):
    with closing(read_only(database)) as conn, conn:
        current = rows(conn)
    by_isrc = {}
    by_id = {row["tidal_id"]: row for row in current}
    for row in current:
        if row["isrc"]:
            by_isrc.setdefault(row["isrc"].strip().upper(), []).append(row)
    candidates = {}
    for source in sources:
        with closing(read_only(source)) as conn, conn:
            historical = rows(conn)
        for old in historical:
            if not old["is_favorite"] or not old["isrc"] or not timestamp(old["date_added"]):
                continue
            matches = by_isrc.get(old["isrc"].strip().upper(), [])
            compatible = [row for row in matches if same_recording(old, row)]
            # Existing IDs can also have had their date overwritten.
            if old["tidal_id"] in by_id:
                compatible = [by_id[old["tidal_id"]]] if same_recording(old, by_id[old["tidal_id"]]) else []
            key = old["tidal_id"]
            if key not in candidates:
                candidates[key] = {"old": old, "sources": [], "matches": compatible,
                    "status": "verified" if len(compatible) == 1 else "review",
                    "reason": "ISRC, artist, title and duration agree" if len(compatible) == 1 else "No unique compatible current recording"}
            item = candidates[key]
            item["sources"].append(str(pathlib.Path(source).resolve()))
            if timestamp(old["date_added"]) < timestamp(item["old"]["date_added"]):
                item["old"] = old
    entries = []
    for item in candidates.values():
        old = item["old"]
        matches = item.pop("matches")
        item["current"] = matches[0] if len(matches) == 1 else None
        item["alternative_candidates"] = matches if len(matches) != 1 else []
        current_row = item["current"]
        item["restore_date"] = bool(current_row and timestamp(current_row["date_added"])
            and timestamp(old["date_added"]) < timestamp(current_row["date_added"]))
        item["restore_alias"] = bool(current_row and old["tidal_id"] != current_row["tidal_id"])
        if item["restore_date"] or item["restore_alias"] or not current_row:
            entries.append(item)
    return {"version": 1, "database": str(pathlib.Path(database).resolve()),
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "changes_favorites": False, "entries": entries}


def apply(database, manifest, backup):
    path = pathlib.Path(database).resolve()
    backup = pathlib.Path(backup).resolve()
    if backup == path or backup.exists():
        raise ValueError("Backup must be a new file distinct from the database")
    entries = manifest["entries"]
    if any(item["status"] != "verified" or not item["current"] for item in entries):
        raise ValueError("Remove review entries from the explicit apply manifest first")
    with closing(sqlite3.connect(path.as_uri() + "?mode=rw", uri=True)) as conn, conn:
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA foreign_keys=ON")
        conn.execute("PRAGMA busy_timeout=5000")
        tables = {row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if not {"tidal_track_aliases", "catalogue_merge_audit"} <= tables:
            raise ValueError("Run the updated application's schema migration before recovery")
        # Backup uses SQLite's snapshot API, including committed WAL contents.
        with closing(sqlite3.connect(backup)) as destination, destination:
            conn.backup(destination)
        count = 0
        conn.execute("BEGIN IMMEDIATE")
        try:
            for item in entries:
                old, expected = item["old"], item["current"]
                row = conn.execute("""SELECT t.*,a.name artist FROM tracks t
                    JOIN artists a ON a.id=t.artist_id WHERE t.id=?""", [expected["id"]]).fetchone()
                if (not row or row["tidal_id"] != expected["tidal_id"]
                        or (row["isrc"] or "").strip().upper() != old["isrc"].strip().upper()
                        or not same_recording(old, dict(row))):
                    raise ValueError("Recording changed since audit; regenerate the manifest")
                owner = conn.execute("SELECT track_id FROM tidal_track_aliases WHERE tidal_id=?", [old["tidal_id"]]).fetchone()
                if owner and owner[0] != row["id"]:
                    raise ValueError("Historical alias now belongs to another recording")
                existing = row["library_added_at"] or row["date_added"]
                restore = timestamp(existing) and timestamp(old["date_added"]) < timestamp(existing)
                missing = owner is None
                if not restore and not missing:
                    continue
                conn.execute("""INSERT INTO catalogue_merge_audit(entity,kept_id,removed_id,snapshot_json)
                    VALUES('recovery',?,?,?)""", [row["id"], old["tidal_id"], json.dumps({
                        "before": {key: row[key] for key in ("tidal_id", "date_added", "library_added_at", "library_date_source", "is_favorite")},
                        "historical": old, "sources": item["sources"]})])
                conn.execute("""INSERT OR IGNORE INTO tidal_track_aliases
                    (tidal_id,track_id,evidence,favorite_created)
                    VALUES(?,?,'historical_backup',?)""", [old["tidal_id"], row["id"], old["date_added"]])
                if restore:
                    conn.execute("""UPDATE tracks SET date_added=?,library_added_at=?,library_date_source='recovered'
                        WHERE id=?""", [old["date_added"], old["date_added"], row["id"]])
                count += 1
            conn.commit()
        except Exception:
            conn.rollback()
            raise
        return count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    inspect = commands.add_parser("audit")
    inspect.add_argument("--database", required=True)
    inspect.add_argument("--source", action="append", required=True)
    inspect.add_argument("--output", required=True)
    repair = commands.add_parser("apply")
    repair.add_argument("--database", required=True)
    repair.add_argument("--manifest", required=True)
    repair.add_argument("--backup", required=True)
    args = parser.parse_args()
    if args.command == "audit":
        result = audit(args.database, args.source)
        pathlib.Path(args.output).write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(json.dumps({"entries": len(result["entries"]), "verified": sum(item["status"] == "verified" for item in result["entries"])}))
    else:
        manifest = json.loads(pathlib.Path(args.manifest).read_text(encoding="utf-8"))
        print(json.dumps({"repaired": apply(args.database, manifest, args.backup), "changed_favorites": False}))


if __name__ == "__main__":
    main()
