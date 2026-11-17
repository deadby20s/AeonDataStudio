//! SQL explorer: ad-hoc DuckDB queries over the workspace's Parquet files.
//!
//! Every dataset is a view `ds_<name>` (columns `id`, `text`, `source`, `meta`; `meta` is JSON
//! text, so `meta->>'lang'` works), and every run's removed documents a view
//! `removed_<run>` (`input_row`, `id`, `source`, `step_index`, `rule`, `detail`, `related_row`,
//! `text`). Queries run in a fresh in-memory DuckDB, using half the CPU cores.
//!
//! Built only with the `sql` feature (on by default), because DuckDB is a large C++ build.

use aeon_shared::{SqlResult, SqlTable};

use crate::error::AppResult;
use crate::workspace::Workspace;

/// Lowercase letters, digits and `_`, so names can be used unquoted.
fn ident(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').chars().take(48).collect()
}

/// Views available to queries, with the Parquet files behind them.
pub fn tables(ws: &Workspace) -> AppResult<Vec<SqlTable>> {
    let catalog = ws.catalog()?;
    let mut out: Vec<SqlTable> = Vec::new();
    let mut taken = std::collections::HashSet::new();
    let mut unique = |base: String| {
        let mut name = base.clone();
        let mut n = 2;
        while !taken.insert(name.clone()) {
            name = format!("{base}_{n}");
            n += 1;
        }
        name
    };
    for d in catalog.list_datasets()? {
        let dir = ws.dataset_dir(&d.id)?;
        if !dir.is_dir() {
            continue;
        }
        let base = match ident(&d.name) {
            n if n.is_empty() => d.id.replace('-', "_"),
            n => n,
        };
        out.push(SqlTable {
            name: unique(format!("ds_{base}")),
            kind: "dataset".into(),
            description: format!("{} · {} documents", d.name, d.doc_count.unwrap_or(0)),
            glob: dir.join("part-*.parquet").display().to_string().replace('\\', "/"),
        });
    }
    for r in catalog.list_runs()? {
        let dir = ws.run_dir(&r.id)?.join("removed");
        if r.state != "completed" || !dir.is_dir() {
            continue;
        }
        let has_files = std::fs::read_dir(&dir)?.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".parquet"));
        if !has_files {
            continue;
        }
        out.push(SqlTable {
            name: unique(format!("removed_{}", ident(r.id.trim_start_matches("run-")))),
            kind: "removed".into(),
            description: format!(
                "Removed by “{}” on {}",
                r.pipeline_name,
                r.input_name.clone().unwrap_or_else(|| "(deleted dataset)".into())
            ),
            glob: dir.join("removed-*.parquet").display().to_string().replace('\\', "/"),
        });
    }
    Ok(out)
}

#[cfg(feature = "sql")]
mod engine {
    use std::sync::Arc;
    use std::time::Instant;

    use aeon_shared::{SqlResult, SqlTable};
    use duckdb::Connection;

    use crate::error::{AppError, AppResult};

    fn err(e: duckdb::Error) -> AppError {
        AppError::InvalidInput(e.to_string())
    }

    pub struct Session {
        conn: Connection,
    }

    impl Session {
        pub fn open(tables: &[SqlTable]) -> AppResult<Self> {
            let conn = Connection::open_in_memory().map_err(err)?;
            let threads = (std::thread::available_parallelism().map_or(2, |n| n.get()) / 2).max(1);
            conn.execute_batch(&format!("SET threads TO {threads};")).map_err(err)?;
            for t in tables {
                let glob = t.glob.replace('\'', "''");
                conn.execute_batch(&format!("CREATE VIEW {} AS SELECT * FROM read_parquet('{glob}');", t.name))
                    .map_err(err)?;
            }
            Ok(Self { conn })
        }

        pub fn interrupt_handle(&self) -> Arc<duckdb::InterruptHandle> {
            self.conn.interrupt_handle()
        }

        /// Runs `sql`. Queries return up to `limit` rows as text; other statements (SET,
        /// CREATE …) are executed and report no rows.
        pub fn run(&self, sql: &str, limit: usize) -> AppResult<SqlResult> {
            let sql = sql.trim().trim_end_matches(';').trim();
            if sql.is_empty() {
                return Err(AppError::InvalidInput("Write a query first".into()));
            }
            let started = Instant::now();
            // Every column as text, so any type can be shown.
            let wrapped = format!("SELECT COLUMNS(*)::VARCHAR FROM ({sql}) AS q LIMIT {}", limit + 1);
            let mut stmt = match self.conn.prepare(&wrapped) {
                Ok(s) => s,
                Err(_) => {
                    // Not a query that can be wrapped (e.g. SET or CREATE): run it as is.
                    self.conn.execute_batch(sql).map_err(err)?;
                    return Ok(SqlResult {
                        columns: vec![],
                        rows: vec![],
                        truncated: false,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        message: Some("Statement executed.".into()),
                    });
                }
            };
            let mut rows_iter = stmt.query([]).map_err(err)?;
            let mut rows: Vec<Vec<Option<String>>> = Vec::new();
            let mut columns: Vec<String> = Vec::new();
            while let Some(row) = rows_iter.next().map_err(err)? {
                if columns.is_empty() {
                    columns = row.as_ref().column_names();
                }
                let n = columns.len();
                let mut values = Vec::with_capacity(n);
                for i in 0..n {
                    values.push(row.get::<_, Option<String>>(i).map_err(err)?);
                }
                rows.push(values);
                if rows.len() > limit {
                    break;
                }
            }
            drop(rows_iter);
            if columns.is_empty() {
                columns = stmt.column_names();
            }
            let truncated = rows.len() > limit;
            rows.truncate(limit);
            Ok(SqlResult { columns, rows, truncated, elapsed_ms: started.elapsed().as_millis() as u64, message: None })
        }
    }
}

#[cfg(feature = "sql")]
pub use engine::Session;

pub const AVAILABLE: bool = cfg!(feature = "sql");

/// Runs a query (blocking). `on_interrupt` receives a handle that cancels it.
#[cfg(feature = "sql")]
pub fn run(
    ws: &Workspace,
    sql: &str,
    limit: usize,
    on_interrupt: impl FnOnce(std::sync::Arc<duckdb::InterruptHandle>),
) -> AppResult<SqlResult> {
    let session = Session::open(&tables(ws)?)?;
    on_interrupt(session.interrupt_handle());
    session.run(sql, limit)
}

#[cfg(not(feature = "sql"))]
pub fn run(_ws: &Workspace, _sql: &str, _limit: usize, _on_interrupt: impl FnOnce(())) -> AppResult<SqlResult> {
    Err(crate::error::AppError::InvalidInput("This build does not include the SQL explorer.".into()))
}

#[cfg(all(test, feature = "sql"))]
mod tests {
    use super::*;
    use crate::dataset::{DatasetWriter, NewDoc, TempDataset, write_manifest};
    use aeon_shared::DatasetManifest;

    #[test]
    fn queries_datasets_by_name_and_meta() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_init(dir.path()).unwrap();
        let tmp = TempDataset::create(&ws.datasets_dir(), "ds-a").unwrap();
        let mut w = DatasetWriter::new(tmp.path()).unwrap();
        for i in 0..50 {
            let meta = if i % 2 == 0 { r#"{"lang":"en"}"# } else { r#"{"lang":"de"}"# };
            w.push(NewDoc { id: &i.to_string(), text: &format!("doc {i}"), source: "s", meta_json: meta }).unwrap();
        }
        let (parts, stats) = w.finish().unwrap();
        let m = DatasetManifest {
            format_version: 1,
            id: "ds-a".into(),
            name: "My Web Data".into(),
            description: None,
            created_at_ms: 0,
            parts,
            stats,
            import: None,
            produced_by: None,
            mix: None,
        };
        write_manifest(tmp.path(), &m).unwrap();
        tmp.commit().unwrap();
        ws.catalog().unwrap().insert_dataset(&crate::import::summary_of(&m)).unwrap();

        let t = tables(&ws).unwrap();
        assert_eq!(t[0].name, "ds_my_web_data");
        let r = run(&ws, "SELECT meta->>'lang' AS lang, count(*) AS n FROM ds_my_web_data GROUP BY 1 ORDER BY 1;", 100, |_| {})
            .unwrap();
        assert_eq!(r.columns, vec!["lang", "n"]);
        assert_eq!(r.rows, vec![vec![Some("de".into()), Some("25".into())], vec![Some("en".into()), Some("25".into())]]);
        let r = run(&ws, "SELECT id FROM ds_my_web_data", 10, |_| {}).unwrap();
        assert!(r.truncated && r.rows.len() == 10);
        assert!(run(&ws, "SELECT * FROM nope", 10, |_| {}).is_err());
        assert!(run(&ws, "SET threads TO 1", 10, |_| {}).unwrap().message.is_some());
    }
}
