"""Read-only whole-library audit and reviewed, stale-safe local recovery.

Audit output is evidence, not permission to repair. `review` explicitly chooses
entries for a version-2 manifest; the updated app consumes that manifest during
its normal patch/sync lifecycle. Neither audit nor recovery contacts TIDAL.
"""
import argparse
import datetime as dt
import hashlib
import json
import pathlib
import re
import sqlite3
from contextlib import closing


def read_only(path, static=False):
    conn = sqlite3.connect(pathlib.Path(path).resolve().as_uri() + ("?mode=ro&immutable=1" if static else "?mode=ro"), uri=True)
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


def canonical(value):
    parsed = timestamp(value)
    if parsed is None:
        raise ValueError("Invalid saved date")
    # Preserve raw evidence separately. SQLite/Python date comparisons use instants.
    return parsed.isoformat().replace("+00:00", "Z")


def normalize(value):
    return re.sub(r"\W+", " ", value.casefold()).strip()


def rows(conn):
    columns = {r[1] for r in conn.execute("PRAGMA table_info(tracks)")}
    optional = ["library_added_at", "library_date_source", "date_choice_at", "catalogue_version", "catalogue_explicit"]
    library = "t.is_library" if "is_library" in columns else "0"
    has_playlists = conn.execute("SELECT 1 FROM sqlite_master WHERE name='playlist_tracks'").fetchone()
    playlist = "EXISTS(SELECT 1 FROM playlist_tracks WHERE track_id=t.id)" if has_playlists else "0"
    fields = ",".join("t." + name if name in columns else "NULL AS " + name for name in optional)
    has_intents = conn.execute("SELECT 1 FROM sqlite_master WHERE name='tidal_favorite_intents'").fetchone()
    revision = "(SELECT revision FROM tidal_favorite_intents WHERE entity='track' AND local_id=t.id)" if has_intents else "NULL"
    return [dict(r) for r in conn.execute(f"""SELECT t.id,t.tidal_id,t.title,t.isrc,t.duration_ms,t.date_added,
        t.is_favorite,{library} AS is_library,{playlist} AS playlist_member,a.name artist,al.title album,al.tidal_id album_tidal_id,
        {fields},{revision} intent_revision FROM tracks t JOIN artists a ON a.id=t.artist_id
        LEFT JOIN albums al ON al.id=t.album_id WHERE t.tidal_id>0""")]


def same_recording(before, after):
    a, b = before["duration_ms"] or 0, after["duration_ms"] or 0
    gap = abs(a - b)
    isrc_a, isrc_b = (before.get("isrc") or "").strip().upper(), (after.get("isrc") or "").strip().upper()
    return (bool(isrc_a) and isrc_a == isrc_b
        and normalize(before["title"]) == normalize(after["title"])
        and normalize(before["artist"]) == normalize(after["artist"])
        and normalize(before.get("catalogue_version") or "") == normalize(after.get("catalogue_version") or "")
        and before.get("catalogue_explicit") == after.get("catalogue_explicit")
        and a > 0 and b > 0 and gap <= 15000 and gap * 100 <= max(a, b) * 8)


def baseline(row):
    return {key: row.get(key) for key in ("date_added", "library_added_at", "library_date_source", "date_choice_at", "intent_revision")}


def audit(database, sources, static_sources=()):
    with closing(read_only(database)) as conn, conn:
        current = rows(conn)
        aliases = {}
        if conn.execute("SELECT 1 FROM sqlite_master WHERE name='tidal_track_aliases'").fetchone():
            aliases = {r[0]: r[1] for r in conn.execute("SELECT tidal_id,track_id FROM tidal_track_aliases")}
    by_local = {r["id"]: r for r in current}
    by_id = {r["tidal_id"]: r for r in current}
    by_id.update({tid: by_local[local] for tid, local in aliases.items() if local in by_local})
    by_isrc = {}
    for row in current:
        if row["isrc"]:
            by_isrc.setdefault(row["isrc"].strip().upper(), []).append(row)
    candidates = {}
    for source in sources:
        with closing(read_only(source, pathlib.Path(source).resolve() in {pathlib.Path(p).resolve() for p in static_sources})) as conn, conn:
            historical = rows(conn)
        for old in historical:
            if not (old["is_favorite"] or old["is_library"] or old["playlist_member"]):
                continue
            existing = by_id.get(old["tidal_id"])
            compatible = ([existing] if same_recording(old, existing) else []) if existing else [r for r in by_isrc.get((old["isrc"] or "").strip().upper(), []) if same_recording(old, r)]
            key = old["tidal_id"]
            if key not in candidates:
                candidates[key] = {"old": old, "historically_favorited": False, "sources": [], "current": compatible[0] if len(compatible) == 1 else None,
                    "alternative_candidates": compatible if len(compatible) != 1 else [], "status": "candidate" if len(compatible) == 1 else "review",
                    "reason": "Unique compatible evidence; intent still requires review" if len(compatible) == 1 else "No unique compatible recording, including same-ID identity conflicts"}
            item = candidates[key]
            item["historically_favorited"] |= bool(old["is_favorite"])
            item["sources"].append(str(pathlib.Path(source).resolve()))
            if timestamp(old["date_added"]) and (not timestamp(item["old"]["date_added"]) or timestamp(old["date_added"]) < timestamp(item["old"]["date_added"])):
                item["old"] = old
    entries = []
    counts = {"historical_curated_ids": len(candidates), "later_date": 0, "historically_liked_now_unliked": 0, "not_curated_now": 0, "no_unique_match": 0, "changed_primary_id": 0, "historically_favorited_ids": sum(item["historically_favorited"] for item in candidates.values()), "historically_favorited_later_date": 0, "historically_favorited_no_unique_match": 0, "historically_favorited_not_curated_now": 0}
    for item in candidates.values():
        old = item["old"]
        existing = by_id.get(old["tidal_id"])
        compatible = ([existing] if same_recording(old, existing) else []) if existing else [r for r in by_isrc.get((old["isrc"] or "").strip().upper(), []) if same_recording(old, r)]
        current_row = compatible[0] if len(compatible) == 1 else None
        item["current"] = current_row
        item["alternative_candidates"] = compatible if len(compatible) != 1 else []
        item["status"] = "candidate" if current_row else "review"
        item["restore_date"] = bool(current_row and timestamp(current_row["date_added"]) and timestamp(old["date_added"]) and timestamp(old["date_added"]) < timestamp(current_row["date_added"]))
        item["restore_alias"] = bool(current_row and old["tidal_id"] != current_row["tidal_id"] and old["tidal_id"] not in by_id)
        item["historically_liked_now_unliked"] = bool(current_row and item["historically_favorited"] and not current_row["is_favorite"])
        item["not_curated_now"] = bool(current_row and not current_row["is_favorite"] and not current_row["is_library"] and not current_row["playlist_member"])
        counts["later_date"] += item["restore_date"]
        counts["historically_favorited_later_date"] += bool(item["restore_date"] and item["historically_favorited"])
        counts["historically_liked_now_unliked"] += item["historically_liked_now_unliked"]
        counts["not_curated_now"] += item["not_curated_now"]
        counts["no_unique_match"] += not bool(current_row)
        counts["historically_favorited_no_unique_match"] += bool(not current_row and item["historically_favorited"])
        counts["historically_favorited_not_curated_now"] += bool(item["not_curated_now"] and item["historically_favorited"])
        counts["changed_primary_id"] += bool(current_row and old["tidal_id"] != current_row["tidal_id"])
        if item["restore_date"] or item["restore_alias"] or item["historically_liked_now_unliked"] or item["not_curated_now"] or not current_row:
            entries.append(item)
    return {"version": 2, "database": str(pathlib.Path(database).resolve()), "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "changes_favorites": False, "counts": counts, "entries": entries}


def review(report, ids):
    entries = [dict(item) for item in report["entries"] if item["old"]["tidal_id"] in ids]
    if len(entries) != len(ids) or any(item["status"] != "candidate" or not item["restore_date"] for item in entries):
        raise ValueError("Every selected ID needs unique identity evidence and an older recorded date")
    for item in entries:
        item["status"] = "reviewed"
        item["expected"] = baseline(item["current"])
        item["accepted_date"] = canonical(item["old"]["date_added"])
    payload = {"version": 2, "reviewed_at": dt.datetime.now(dt.timezone.utc).isoformat(), "changes_favorites": False, "entries": entries}
    payload["batch_id"] = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
    return payload


def same_instant(left, right):
    return left == right or bool(timestamp(left) and timestamp(left) == timestamp(right))


def apply(database, manifest, backup):
    path, backup = pathlib.Path(database).resolve(), pathlib.Path(backup).resolve()
    if backup == path or backup.exists():
        raise ValueError("Backup must be a new file distinct from the database")
    if manifest.get("version") != 2 or not manifest.get("batch_id") or not timestamp(manifest.get("reviewed_at")) or manifest.get("changes_favorites") is not False:
        raise ValueError("An explicitly reviewed version-2 manifest is required")
    entries = manifest["entries"]
    if any(item["status"] != "reviewed" or not item["current"] for item in entries):
        raise ValueError("Unreviewed entries cannot be applied")
    with closing(sqlite3.connect(path.as_uri() + "?mode=rw", uri=True)) as conn, conn:
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA foreign_keys=ON")
        conn.execute("PRAGMA busy_timeout=5000")
        columns = {r[1] for r in conn.execute("PRAGMA table_info(tracks)")}
        if "date_choice_at" not in columns:
            raise ValueError("Run the updated application's migration before recovery")
        with closing(sqlite3.connect(backup)) as destination, destination:
            conn.backup(destination)
        count = 0
        conn.execute("BEGIN IMMEDIATE")
        try:
            current = {r["id"]: r for r in rows(conn)}
            for item in entries:
                old, expected = item["old"], item["current"]
                row = current.get(expected["id"])
                if not row or row["tidal_id"] != expected["tidal_id"] or not same_recording(old, row):
                    raise ValueError("Recording changed since audit; regenerate the manifest")
                owner = conn.execute("SELECT track_id FROM tidal_track_aliases WHERE tidal_id=?", [old["tidal_id"]]).fetchone()
                if owner and owner[0] != row["id"]:
                    raise ValueError("Historical alias now belongs to another recording")
                marker = json.dumps({"batch_id": manifest["batch_id"], "historical_id": old["tidal_id"]}, sort_keys=True)
                prior = conn.execute("SELECT 1 FROM catalogue_merge_audit WHERE entity='recovery' AND json_extract(snapshot_json,'$.marker')=?", [marker]).fetchone()
                if prior:
                    # A later re-like is retained even when the same batch is retried.
                    continue
                observed, wanted = baseline(row), item["expected"]
                for name in ("date_added", "library_added_at", "date_choice_at"):
                    if name == "library_added_at" and wanted[name] is None:
                        wanted = dict(wanted, library_added_at=wanted["date_added"])
                    if not same_instant(observed[name], wanted[name]):
                        raise ValueError("Saved date choice changed since review")
                if observed["intent_revision"] != wanted["intent_revision"] or (observed["library_date_source"] != wanted["library_date_source"] and not (wanted["library_date_source"] is None and observed["library_date_source"] == "legacy")):
                    raise ValueError("User intent or provenance changed since review")
                date = canonical(item["accepted_date"])
                if timestamp(date) != timestamp(old["date_added"]) or timestamp(date) >= timestamp(row["date_added"]):
                    raise ValueError("Accepted date must be the older evidenced date")
                conn.execute("INSERT INTO catalogue_merge_audit(entity,kept_id,removed_id,snapshot_json) VALUES('recovery',?,?,?)", [row["id"], old["tidal_id"], json.dumps({"marker": marker, "before": row, "historical": old, "sources": item["sources"]})])
                conn.execute("INSERT OR IGNORE INTO tidal_track_aliases(tidal_id,track_id,evidence,favorite_created) VALUES(?,?,'historical_backup',?)", [old["tidal_id"], row["id"], old["date_added"]])
                conn.execute("UPDATE tracks SET date_added=?,library_added_at=?,library_date_source='recovered',date_choice_at=? WHERE id=?", [date, date, manifest["reviewed_at"], row["id"]])
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
    inspect.add_argument("--source", action="append", default=[])
    inspect.add_argument("--static-source", action="append", default=[], help="Known closed historical backup artifacts only; never a live database")
    inspect.add_argument("--output", required=True)
    reviewed = commands.add_parser("review")
    reviewed.add_argument("--audit", required=True)
    reviewed.add_argument("--id", type=int, action="append", required=True)
    reviewed.add_argument("--output", required=True)
    repair = commands.add_parser("apply")
    repair.add_argument("--database", required=True)
    repair.add_argument("--manifest", required=True)
    repair.add_argument("--backup", required=True)
    args = parser.parse_args()
    if args.command == "audit":
        if not args.source and not args.static_source:
            parser.error("At least one historical source is required")
        result = audit(args.database, args.source + args.static_source, args.static_source)
    elif args.command == "review":
        result = review(json.loads(pathlib.Path(args.audit).read_text(encoding="utf-8")), set(args.id))
    else:
        manifest = json.loads(pathlib.Path(args.manifest).read_text(encoding="utf-8"))
        print(json.dumps({"repaired": apply(args.database, manifest, args.backup), "changed_favorites": False}))
        return
    pathlib.Path(args.output).write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"entries": len(result["entries"]), "counts": result.get("counts")}))


if __name__ == "__main__":
    main()
