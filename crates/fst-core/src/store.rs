use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn atom_id(raw_content: &str) -> [u8; 32] {
    Sha256::digest(raw_content.as_bytes()).into()
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS atoms (
    id          BLOB PRIMARY KEY CHECK (length(id) = 32),
    raw_content TEXT NOT NULL,
    prettier    TEXT NOT NULL DEFAULT ''
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS atom_methods (
    atom_id BLOB NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,
    method  TEXT NOT NULL,
    PRIMARY KEY (atom_id, method)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_methods_method ON atom_methods(method);

CREATE TABLE IF NOT EXISTS methods (
    name TEXT PRIMARY KEY
) WITHOUT ROWID;

INSERT OR IGNORE INTO methods (name)
    SELECT DISTINCT method FROM atom_methods;
";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub struct Store {
    pub conn: Connection,
}

#[derive(Debug, serde::Serialize)]
pub struct Atom {
    pub id: String,
    pub raw_content: String,
    pub methods: Vec<String>,
    pub prettier: String,
}

pub struct FoundAtom {
    pub raw_content: String,
    pub methods: Vec<String>,
}

#[derive(Debug, Default, PartialEq, serde::Serialize)]
pub struct SyncReport {
    pub added: usize,
    pub removed: usize,
    pub total: usize,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;",
        )?;
        Ok(Self { conn })
    }

    pub fn migrate(&self) -> Result<(), StoreError> {
        self.conn.execute_batch(SCHEMA)?;
        Ok(())
    }

    // methods
    pub fn all_methods(&self) -> Result<Vec<String>, StoreError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT name FROM methods ORDER BY name")?;
        let methods = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(methods)
    }

    // atoms
    pub fn insert_atom(
        &mut self,
        raw_content: &str,
        methods: &[&str],
    ) -> Result<String, StoreError> {
        let id = atom_id(raw_content);
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO atoms (id, raw_content) VALUES (?1, ?2)",
            rusqlite::params![&id[..], raw_content],
        )?;
        {
            let mut link = tx.prepare_cached(
                "INSERT OR IGNORE INTO atom_methods (atom_id, method) VALUES (?1, ?2)",
            )?;
            let mut reg = tx.prepare_cached("INSERT OR IGNORE INTO methods (name) VALUES (?1)")?;
            for m in methods {
                reg.execute([m])?;
                link.execute(rusqlite::params![&id[..], m])?;
            }
        }
        tx.commit()?;
        Ok(hex::encode(id))
    }

    pub fn get_atom(&self, id_hex: &str) -> Result<Option<Atom>, StoreError> {
        let Ok(id) = hex::decode(id_hex) else {
            return Ok(None);
        };
        let row = self.conn.query_row(
            "SELECT raw_content, prettier FROM atoms WHERE id = ?1",
            [&id[..]],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        );
        let (raw_content, prettier) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut stmt = self
            .conn
            .prepare_cached("SELECT method FROM atom_methods WHERE atom_id = ?1 ORDER BY method")?;
        let methods = stmt
            .query_map([&id[..]], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(Some(Atom {
            id: id_hex.to_string(),
            raw_content,
            methods,
            prettier,
        }))
    }

    pub fn atoms_by_method(
        &self,
        method: &str,
        after_hex: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Atom>, StoreError> {
        let after = after_hex
            .and_then(|h| hex::decode(h).ok())
            .unwrap_or_default(); // empty blob sorts before every id

        let mut stmt = self.conn.prepare_cached(
            "SELECT atom_id FROM atom_methods
             WHERE method = ?1 AND atom_id > ?2
             ORDER BY atom_id LIMIT ?3",
        )?;
        let ids = stmt
            .query_map(rusqlite::params![method, &after[..], limit], |r| {
                r.get::<_, Vec<u8>>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(a) = self.get_atom(&hex::encode(id))? {
                out.push(a);
            }
        }
        Ok(out)
    }

    pub fn sync_atoms(&mut self, found: &[FoundAtom]) -> Result<SyncReport, StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS found (id BLOB PRIMARY KEY) WITHOUT ROWID;
             DELETE FROM found;",
        )?;

        let mut report = SyncReport::default();
        {
            let mut ins_found =
                tx.prepare_cached("INSERT OR IGNORE INTO found (id) VALUES (?1)")?;
            let mut ins_atom =
                tx.prepare_cached("INSERT OR IGNORE INTO atoms (id, raw_content) VALUES (?1, ?2)")?;
            let mut reg = tx.prepare_cached("INSERT OR IGNORE INTO methods (name) VALUES (?1)")?;
            let mut link = tx.prepare_cached(
                "INSERT OR IGNORE INTO atom_methods (atom_id, method) VALUES (?1, ?2)",
            )?;

            for a in found {
                let id = atom_id(&a.raw_content);
                ins_found.execute([&id[..]])?;
                report.added += ins_atom.execute(rusqlite::params![&id[..], a.raw_content])?;
                for m in &a.methods {
                    reg.execute([m])?;
                    link.execute(rusqlite::params![&id[..], m])?;
                }
            }
        }

        report.removed = tx.execute(
            "DELETE FROM atoms WHERE id NOT IN (SELECT id FROM found)",
            [],
        )?;
        tx.execute(
            "DELETE FROM methods WHERE name NOT IN (SELECT method FROM atom_methods)",
            [],
        )?;
        report.total =
            tx.query_row("SELECT count(*) FROM atoms", [], |r| r.get::<_, i64>(0))? as usize;

        tx.commit()?;
        Ok(report)
    }

    pub fn all_atoms(&self, after_hex: Option<&str>, limit: u32) -> Result<Vec<Atom>, StoreError> {
        let after = after_hex
            .and_then(|h| hex::decode(h).ok())
            .unwrap_or_default(); // empty blob sorts before every id

        let mut stmt = self.conn.prepare_cached(
            "SELECT id, raw_content, prettier FROM atoms
             WHERE id > ?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![&after[..], limit], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut methods_stmt = self
            .conn
            .prepare_cached("SELECT method FROM atom_methods WHERE atom_id = ?1 ORDER BY method")?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, raw_content, prettier) in rows {
            let methods = methods_stmt
                .query_map([&id[..]], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            out.push(Atom {
                id: hex::encode(&id),
                raw_content,
                methods,
                prettier,
            });
        }
        Ok(out)
    }
}
