import copy
import json
import pathlib
import re
import sqlite3
from contextlib import closing
import uuid
import shutil
import unittest

import tidal_catalogue_recovery as recovery


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        test_root = pathlib.Path(__file__).resolve().parents[1] / "target/catalogue-recovery/tests"
        test_root.mkdir(parents=True, exist_ok=True)
        self.root = test_root / str(uuid.uuid4())
        self.root.mkdir()
        self.addCleanup(shutil.rmtree, self.root)
        self.current = self.root / "current.db"
        self.old = self.root / "old.db"
        schema = """
            CREATE TABLE artists(id INTEGER PRIMARY KEY,name TEXT,tidal_id INTEGER);
            CREATE TABLE albums(id INTEGER PRIMARY KEY,tidal_id INTEGER,title TEXT,is_favorite INTEGER);
            CREATE TABLE tracks(id INTEGER PRIMARY KEY,tidal_id INTEGER,title TEXT,isrc TEXT,
                artist_id INTEGER,album_id INTEGER,duration_ms INTEGER,date_added TEXT,
                is_favorite INTEGER,is_library INTEGER);
            INSERT INTO artists VALUES(1,'The Chats',1);
            INSERT INTO albums VALUES(1,1,'Get This In Ya',1);
        """
        for path, tid, date in [(self.old, 122611523, "2020-06-10T07:10:44.221+0000"),
                                (self.current, 556255961, "2026-09-18 07:37:31")]:
            with closing(sqlite3.connect(path)) as conn, conn:
                conn.executescript(schema)
                conn.execute("INSERT INTO tracks VALUES(1,?,'Smoko','AUBEC1712176',1,1,180000,?,1,1)", [tid, date])
        self.manifest = recovery.audit(self.current, [self.old])
        source = (pathlib.Path(__file__).resolve().parents[1] / "noor-server/src/db/schema.rs").read_text(encoding="utf-8")
        migration = re.search(r'pub\(crate\) const MIGRATION_069: &str = r#"(.*?)"#;', source, re.S).group(1)
        with closing(sqlite3.connect(self.current)) as conn, conn:
            conn.executescript(migration)

    def test_restores_date_and_alias_without_changing_favorites_and_repeats_safely(self):
        self.assertEqual(recovery.apply(self.current, self.manifest, self.root / "backup.db"), 1)
        self.assertEqual(recovery.apply(self.current, self.manifest, self.root / "repeat.db"), 0)
        with closing(sqlite3.connect(self.current)) as conn, conn:
            self.assertEqual(conn.execute("SELECT date_added,is_favorite FROM tracks").fetchone(),
                             ("2020-06-10T07:10:44.221+0000", 1))
            self.assertEqual(conn.execute("SELECT track_id FROM tidal_track_aliases WHERE tidal_id=122611523").fetchone(), (1,))
            self.assertEqual(conn.execute("SELECT COUNT(*) FROM catalogue_merge_audit").fetchone(), (1,))
        with closing(sqlite3.connect(self.root / "backup.db")) as conn, conn:
            self.assertEqual(conn.execute("SELECT date_added FROM tracks").fetchone(), ("2026-09-18 07:37:31",))

    def test_later_conflict_rolls_back_entire_repair(self):
        manifest = copy.deepcopy(self.manifest)
        invalid = copy.deepcopy(manifest["entries"][0])
        invalid["current"]["id"] = 999
        manifest["entries"].append(invalid)
        with self.assertRaises(ValueError):
            recovery.apply(self.current, manifest, self.root / "backup.db")
        with closing(sqlite3.connect(self.current)) as conn, conn:
            self.assertEqual(conn.execute("SELECT date_added FROM tracks").fetchone(), ("2026-09-18 07:37:31",))
            self.assertEqual(conn.execute("SELECT COUNT(*) FROM catalogue_merge_audit").fetchone(), (0,))
            self.assertEqual(conn.execute("SELECT COUNT(*) FROM tidal_track_aliases WHERE tidal_id=122611523").fetchone(), (0,))

    def test_audit_does_not_change_source_and_marks_ambiguous_matches_for_review(self):
        with closing(sqlite3.connect(self.current)) as conn, conn:
            conn.execute("INSERT INTO tracks(id,tidal_id,title,isrc,artist_id,album_id,duration_ms,date_added,is_favorite,is_library) VALUES(2,777,'Smoko','AUBEC1712176',1,1,180000,'2026-09-18 07:37:31',0,1)")
        before = self.old.read_bytes()
        manifest = recovery.audit(self.current, [self.old])
        self.assertEqual(manifest["entries"][0]["status"], "review")
        self.assertEqual(self.old.read_bytes(), before)
        with self.assertRaises(ValueError):
            recovery.apply(self.current, manifest, self.root / "backup.db")

    def test_backup_includes_wal_transactions(self):
        with closing(sqlite3.connect(self.current)) as writer, writer:
            writer.execute("PRAGMA journal_mode=WAL")
            writer.execute("UPDATE tracks SET is_favorite=0")
            writer.commit()
            visible = recovery.audit(self.current, [self.old])
            self.assertEqual(visible["entries"][0]["current"]["is_favorite"], 0)
            recovery.apply(self.current, self.manifest, self.root / "backup.db")
            with closing(sqlite3.connect(self.root / "backup.db")) as conn, conn:
                self.assertEqual(conn.execute("SELECT is_favorite FROM tracks").fetchone(), (0,))


if __name__ == "__main__":
    unittest.main()
