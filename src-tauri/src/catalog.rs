//! SQLite catalog of datasets and runs, stored as `catalog.sqlite` in the workspace.

use std::path::Path;

use aeon_shared::DatasetSummary;
use rusqlite::Connection;

use crate::error::AppResult;

/// Schema migrations, applied in order. `PRAGMA user_version` records how many have run.
/// Never edit an existing entry; append a new one.
const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r#"
    CREATE TABLE datasets (
        id            TEXT PRIMARY KEY,
        name          TEXT NOT NULL,
        description   TEXT,
        created_at_ms INTEGER NOT NULL,
        doc_count     INTEGER,
        byte_size     INTEGER,
        parent_run_id TEXT REFERENCES runs(id)
    );
    CREATE TABLE runs (
        id                TEXT PRIMARY KEY,
        pipeline_name     TEXT NOT NULL,
        input_dataset_id  TEXT REFERENCES datasets(id),
        output_dataset_id TEXT REFERENCES datasets(id),
        state             TEXT NOT NULL,
        started_at_ms     INTEGER NOT NULL,
        finished_at_ms    INTEGER
    );
    "#,
];

pub struct Catalog {
    conn: Connection,
}

impl Catalog {
    pub fn open(path: &Path) -> AppResult<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> AppResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> AppResult<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn dataset_count(&self) -> AppResult<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM datasets", [], |r| r.get::<_, i64>(0))? as u64)
    }

    pub fn run_count(&self) -> AppResult<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get::<_, i64>(0))? as u64)
    }

    pub fn list_datasets(&self) -> AppResult<Vec<DatasetSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, created_at_ms, doc_count, byte_size, parent_run_id
             FROM datasets ORDER BY created_at_ms DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(DatasetSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                created_at_ms: r.get(3)?,
                doc_count: r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                byte_size: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                parent_run_id: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    #[cfg(test)]
    pub fn insert_dataset(&self, d: &DatasetSummary) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO datasets (id, name, description, created_at_ms, doc_count, byte_size, parent_run_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                d.id,
                d.name,
                d.description,
                d.created_at_ms,
                d.doc_count.map(|v| v as i64),
                d.byte_size.map(|v| v as i64),
                d.parent_run_id,
            ],
        )?;
        Ok(())
    }

    #[cfg(test)]
    fn schema_version(&self) -> AppResult<i64> {
        Ok(self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }
}

fn migrate(conn: &mut Connection) -> AppResult<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_lists_datasets() {
        let cat = Catalog::open_in_memory().unwrap();
        assert_eq!(cat.schema_version().unwrap(), MIGRATIONS.len() as i64);
        assert_eq!(cat.dataset_count().unwrap(), 0);

        let older = DatasetSummary {
            id: "a".into(),
            name: "older".into(),
            description: None,
            created_at_ms: 1,
            doc_count: Some(10),
            byte_size: None,
            parent_run_id: None,
        };
        let newer = DatasetSummary { id: "b".into(), name: "newer".into(), created_at_ms: 2, ..older.clone() };
        cat.insert_dataset(&older).unwrap();
        cat.insert_dataset(&newer).unwrap();

        let list = cat.list_datasets().unwrap();
        assert_eq!(list, vec![newer, older]);
        assert_eq!(cat.dataset_count().unwrap(), 2);
    }

    #[test]
    fn reopening_does_not_rerun_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        drop(Catalog::open(&path).unwrap());
        let cat = Catalog::open(&path).unwrap();
        assert_eq!(cat.schema_version().unwrap(), MIGRATIONS.len() as i64);
    }
}
