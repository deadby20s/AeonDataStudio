//! SQLite catalog of datasets and runs, stored as `catalog.sqlite` in the workspace.

use std::path::Path;

use aeon_shared::{DatasetSummary, RunSummary};
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
    // 2: totals and a short source description for the dataset list
    r#"
    ALTER TABLE datasets ADD COLUMN char_count INTEGER;
    ALTER TABLE datasets ADD COLUMN source_desc TEXT;
    "#,
    // 3: run totals and failure message
    r#"
    ALTER TABLE runs ADD COLUMN docs_in INTEGER;
    ALTER TABLE runs ADD COLUMN docs_out INTEGER;
    ALTER TABLE runs ADD COLUMN error TEXT;
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
            "SELECT id, name, description, created_at_ms, doc_count, char_count, byte_size, source_desc,
                    parent_run_id
             FROM datasets ORDER BY created_at_ms DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(DatasetSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                created_at_ms: r.get(3)?,
                doc_count: r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                char_count: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                byte_size: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                source_desc: r.get(7)?,
                parent_run_id: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn insert_dataset(&self, d: &DatasetSummary) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO datasets (id, name, description, created_at_ms, doc_count, char_count, byte_size,
                                   source_desc, parent_run_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                d.id,
                d.name,
                d.description,
                d.created_at_ms,
                d.doc_count.map(|v| v as i64),
                d.char_count.map(|v| v as i64),
                d.byte_size.map(|v| v as i64),
                d.source_desc,
                d.parent_run_id,
            ],
        )?;
        Ok(())
    }

    /// Returns whether a row was deleted. Runs that used the dataset keep their history but
    /// lose the link.
    pub fn delete_dataset(&self, id: &str) -> AppResult<bool> {
        self.conn.execute("UPDATE runs SET input_dataset_id = NULL WHERE input_dataset_id = ?1", [id])?;
        self.conn.execute("UPDATE runs SET output_dataset_id = NULL WHERE output_dataset_id = ?1", [id])?;
        Ok(self.conn.execute("DELETE FROM datasets WHERE id = ?1", [id])? > 0)
    }

    pub fn insert_run(&self, id: &str, pipeline_name: &str, input_dataset_id: &str, started_at_ms: i64) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO runs (id, pipeline_name, input_dataset_id, state, started_at_ms) VALUES (?1, ?2, ?3, 'running', ?4)",
            rusqlite::params![id, pipeline_name, input_dataset_id, started_at_ms],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_run(
        &self,
        id: &str,
        state: &str,
        output_dataset_id: Option<&str>,
        docs_in: Option<u64>,
        docs_out: Option<u64>,
        finished_at_ms: i64,
        error: Option<&str>,
    ) -> AppResult<()> {
        self.conn.execute(
            "UPDATE runs SET state = ?2, output_dataset_id = ?3, docs_in = ?4, docs_out = ?5, finished_at_ms = ?6,
                             error = ?7
             WHERE id = ?1",
            rusqlite::params![
                id,
                state,
                output_dataset_id,
                docs_in.map(|v| v as i64),
                docs_out.map(|v| v as i64),
                finished_at_ms,
                error
            ],
        )?;
        Ok(())
    }

    /// Runs left "running" by a crash are marked failed.
    pub fn fail_interrupted_runs(&self, now_ms: i64) -> AppResult<usize> {
        Ok(self.conn.execute(
            "UPDATE runs SET state = 'failed', finished_at_ms = ?1, error = 'Interrupted (the app closed during the run)'
             WHERE state = 'running'",
            [now_ms],
        )?)
    }

    pub fn list_runs(&self) -> AppResult<Vec<RunSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.pipeline_name, r.input_dataset_id, di.name, r.output_dataset_id, dout.name, r.state,
                    r.started_at_ms, r.finished_at_ms, r.docs_in, r.docs_out, r.error
             FROM runs r
             LEFT JOIN datasets di ON di.id = r.input_dataset_id
             LEFT JOIN datasets dout ON dout.id = r.output_dataset_id
             ORDER BY r.started_at_ms DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(RunSummary {
                id: r.get(0)?,
                pipeline_name: r.get(1)?,
                input_dataset_id: r.get(2)?,
                input_name: r.get(3)?,
                output_dataset_id: r.get(4)?,
                output_name: r.get(5)?,
                state: r.get(6)?,
                started_at_ms: r.get(7)?,
                finished_at_ms: r.get(8)?,
                docs_in: r.get::<_, Option<i64>>(9)?.map(|v| v as u64),
                docs_out: r.get::<_, Option<i64>>(10)?.map(|v| v as u64),
                error: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Deletes the run record. Datasets it produced are kept but lose the link.
    pub fn delete_run(&self, id: &str) -> AppResult<bool> {
        self.conn.execute("UPDATE datasets SET parent_run_id = NULL WHERE parent_run_id = ?1", [id])?;
        Ok(self.conn.execute("DELETE FROM runs WHERE id = ?1", [id])? > 0)
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
            char_count: Some(100),
            byte_size: None,
            source_desc: Some("1 JSONL file".into()),
            parent_run_id: None,
        };
        let newer = DatasetSummary { id: "b".into(), name: "newer".into(), created_at_ms: 2, ..older.clone() };
        cat.insert_dataset(&older).unwrap();
        cat.insert_dataset(&newer).unwrap();

        let list = cat.list_datasets().unwrap();
        assert_eq!(list, vec![newer.clone(), older]);
        assert_eq!(cat.dataset_count().unwrap(), 2);

        assert!(cat.delete_dataset("a").unwrap());
        assert!(!cat.delete_dataset("a").unwrap());
        assert_eq!(cat.list_datasets().unwrap(), vec![newer]);
    }

    #[test]
    fn runs_link_datasets_and_survive_their_deletion() {
        let cat = Catalog::open_in_memory().unwrap();
        let input = DatasetSummary {
            id: "in".into(),
            name: "Input".into(),
            description: None,
            created_at_ms: 1,
            doc_count: Some(10),
            char_count: None,
            byte_size: None,
            source_desc: None,
            parent_run_id: None,
        };
        cat.insert_dataset(&input).unwrap();
        cat.insert_run("r1", "Clean", "in", 5).unwrap();
        let output = DatasetSummary { id: "out".into(), name: "Output".into(), parent_run_id: Some("r1".into()), ..input.clone() };
        cat.insert_dataset(&output).unwrap();
        cat.finish_run("r1", "completed", Some("out"), Some(10), Some(7), 9, None).unwrap();

        let runs = cat.list_runs().unwrap();
        assert_eq!(runs[0].input_name.as_deref(), Some("Input"));
        assert_eq!(runs[0].output_name.as_deref(), Some("Output"));
        assert_eq!((runs[0].docs_in, runs[0].docs_out), (Some(10), Some(7)));

        // Foreign keys are on: deleting linked rows must not fail.
        assert!(cat.delete_dataset("in").unwrap());
        assert!(cat.delete_run("r1").unwrap());
        assert_eq!(cat.list_datasets().unwrap()[0].parent_run_id, None);

        cat.insert_run("r2", "Clean", "out", 6).unwrap();
        assert_eq!(cat.fail_interrupted_runs(7).unwrap(), 1);
        assert_eq!(cat.list_runs().unwrap()[0].state, "failed");
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
