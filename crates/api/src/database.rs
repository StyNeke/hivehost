use rusqlite::{Connection, TransactionBehavior};
use std::{path::Path, time::Duration};

const SCHEMA_VERSION: i64 = 9;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if version == SCHEMA_VERSION {
        return Ok(connection);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }
    transaction.execute_batch(concat!(
        include_str!("../migrations/0001_jobs.sql"),
        "\n",
        include_str!("../migrations/0002_auth.sql"),
        "\n",
        include_str!("../migrations/0003_eggs.sql"),
        "\n",
        include_str!("../migrations/0004_servers.sql"),
        "\n",
        include_str!("../migrations/0005_admission.sql"),
        "\n",
        include_str!("../migrations/0006_runtime.sql"),
        "\n",
        include_str!("../migrations/0007_backups.sql"),
        "\n",
        include_str!("../migrations/0008_schedules.sql"),
        "\n",
        include_str!("../migrations/0009_experimental.sql"),
    ))?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(connection)
}
