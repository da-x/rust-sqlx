use anyhow::{bail, Context};
use sqlx::{AnyConnection, Executor, Row};
use std::process::Command;

/// Dump the database schema as SQL, excluding the `_sqlx_migrations` table.
pub async fn dump_schema(conn: &mut AnyConnection, database_url: &str) -> anyhow::Result<String> {
    let backend = conn.backend_name();

    let dump = match backend {
        "PostgreSQL" => dump_postgres(database_url)?,
        "MySQL" => dump_mysql(database_url)?,
        "SQLite" => dump_sqlite(conn).await?,
        other => bail!("schema dump is not supported for database backend {other}"),
    };

    Ok(filter_sqlx_migrations_statements(&dump))
}

fn dump_postgres(database_url: &str) -> anyhow::Result<String> {
    let output = Command::new("pg_dump")
        .args([
            "--schema-only",
            "--no-owner",
            "--no-acl",
            "--exclude-table=_sqlx_migrations",
            database_url,
        ])
        .output()
        .context(
            "failed to run `pg_dump`; install PostgreSQL client tools or ensure it is on PATH",
        )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("pg_dump failed: {stderr}");
    }

    String::from_utf8(output.stdout).context("pg_dump output was not valid UTF-8")
}

fn dump_mysql(database_url: &str) -> anyhow::Result<String> {
    // mysqldump does not accept a full URL; parse components.
    let url = url::Url::parse(database_url).context("invalid DATABASE_URL for MySQL dump")?;
    let host = url.host_str().unwrap_or("localhost");
    let port = url.port().unwrap_or(3306);
    let user = if url.username().is_empty() {
        "root"
    } else {
        url.username()
    };
    let password = url.password().unwrap_or("");
    let database = url
        .path()
        .trim_start_matches('/')
        .split('?')
        .next()
        .filter(|s| !s.is_empty())
        .context("DATABASE_URL must include a database name for MySQL dump")?;

    let ignore_table = format!("{database}._sqlx_migrations");

    let mut cmd = Command::new("mysqldump");
    cmd.args([
        "--no-data",
        "--routines",
        "--triggers",
        "--skip-comments",
        "--set-gtid-purged=OFF",
        "-h",
        host,
        "-P",
        &port.to_string(),
        "-u",
        user,
        &format!("--ignore-table={ignore_table}"),
        database,
    ]);

    if !password.is_empty() {
        cmd.arg(format!("--password={password}"));
    }

    let output = cmd.output().context(
        "failed to run `mysqldump`; install MySQL client tools or ensure it is on PATH",
    )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("mysqldump failed: {stderr}");
    }

    String::from_utf8(output.stdout).context("mysqldump output was not valid UTF-8")
}

async fn dump_sqlite(conn: &mut AnyConnection) -> anyhow::Result<String> {
    // language=SQLite
    let rows = conn
        .fetch_all(
            r#"
SELECT sql FROM sqlite_master
WHERE sql IS NOT NULL
  AND name != '_sqlx_migrations'
  AND name NOT LIKE 'sqlite_%'
ORDER BY
  CASE type
    WHEN 'table' THEN 1
    WHEN 'index' THEN 2
    WHEN 'view' THEN 3
    WHEN 'trigger' THEN 4
    ELSE 5
  END,
  name
            "#,
        )
        .await
        .context("failed to read sqlite_master for schema dump")?;

    let mut out = String::new();
    for row in rows {
        let sql: String = row.try_get(0)?;
        if sql.contains("_sqlx_migrations") {
            continue;
        }
        out.push_str(sql.trim());
        if !sql.trim_end().ends_with(';') {
            out.push(';');
        }
        out.push('\n');
    }

    Ok(out)
}

/// Drop CREATE/DROP statements that still mention `_sqlx_migrations` (belt and suspenders).
fn filter_sqlx_migrations_statements(dump: &str) -> String {
    dump.lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            !lower.contains("_sqlx_migrations")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
