use anyhow::{Result, ensure};
use cordis_wasm::Storage;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::{path::Path, sync::Mutex};

/// Host-owned, plugin-namespaced durable JSON documents. No issue/calendar logic.
pub struct Database(Mutex<Connection>);
impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(version <= 1, "database schema is newer than this host");
        connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS documents(namespace TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(namespace,key)); PRAGMA user_version=1;")?;
        Ok(Self(Mutex::new(connection)))
    }
}
impl Storage for Database {
    fn execute(&self, owner: &str, operation: &str, payload: &str) -> Result<String> {
        let mut connection = self.0.lock().unwrap();
        match operation {
            "scan" => {
                let mut stmt = connection
                    .prepare("SELECT value FROM documents WHERE namespace=?1 ORDER BY key")?;
                let rows = stmt.query_map([owner], |row| row.get::<_, String>(0))?;
                let mut values = Vec::<Value>::new();
                let mut bytes = 0;
                for row in rows {
                    let row = row?;
                    bytes += row.len();
                    ensure!(
                        bytes <= cordis_wire::MAX_BYTES && values.len() < 2048,
                        "scan limit; pagination required"
                    );
                    values.push(serde_json::from_str(&row)?);
                }
                Ok(serde_json::to_string(&values)?)
            }
            "batch" => {
                let operations: Vec<Value> = serde_json::from_str(payload)?;
                ensure!(operations.len() <= 256, "transaction operation limit");
                let transaction = connection.transaction()?;
                for op in operations {
                    let key = op["key"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("key must be a string"))?;
                    ensure!(key.len() <= 128, "key limit");
                    if op["delete"].as_bool() == Some(true) {
                        transaction.execute(
                            "DELETE FROM documents WHERE namespace=?1 AND key=?2",
                            params![owner, key],
                        )?;
                        continue;
                    }
                    ensure!(op.get("value").is_some(), "value required");
                    transaction.execute("INSERT INTO documents(namespace,key,value) VALUES(?1,?2,?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",params![owner,key,op["value"].to_string()])?;
                }
                transaction.commit()?;
                Ok("null".into())
            }
            _ => anyhow::bail!("unknown storage operation"),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batch_rollback_and_namespace_isolation() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.execute("a", "batch", r#"[{"key":"1","value":{"title":"saved"}}]"#)
            .unwrap();
        assert!(
            db.execute(
                "a",
                "batch",
                r#"[{"key":"2","value":true},{"value":false}]"#
            )
            .is_err()
        );
        assert_eq!(db.execute("b", "scan", "{}").unwrap(), "[]");
        assert_eq!(
            serde_json::from_str::<Vec<Value>>(&db.execute("a", "scan", "{}").unwrap())
                .unwrap()
                .len(),
            1
        );
    }
}
