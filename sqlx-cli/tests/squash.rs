use assert_cmd::Command;
use sqlx::{migrate::Migrate, Connection, SqliteConnection};
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

fn write_migrations(dir: &std::path::Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("20220101000000_create.sql"),
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL);\n\
         INSERT INTO items (id, name) VALUES (1, 'a');\n",
    )
    .unwrap();
    fs::write(
        dir.join("20220102000000_add_col.sql"),
        "ALTER TABLE items ADD COLUMN qty INTEGER NOT NULL DEFAULT 0;\n",
    )
    .unwrap();
}

#[tokio::test]
async fn squash_then_run_rewrites_history() {
    let tmp = tempdir().unwrap();
    let migrations = tmp.path().join("migrations");
    write_migrations(&migrations);

    let db_path = tmp.path().join("test.db");
    let db_url = format!("sqlite://{}", db_path.display());

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args(["sqlx", "database", "create", "--database-url", &db_url])
        .assert()
        .success();

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args([
            "sqlx",
            "migrate",
            "run",
            "--database-url",
            &db_url,
            "--source",
            migrations.to_str().unwrap(),
        ])
        .assert()
        .success();

    let mut conn = SqliteConnection::connect(&db_url).await.unwrap();
    let before = conn.list_applied_migrations().await.unwrap();
    assert_eq!(before.len(), 2);
    drop(conn);

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args([
            "sqlx",
            "migrate",
            "squash",
            "--database-url",
            &db_url,
            "--source",
            migrations.to_str().unwrap(),
            "--no-commit",
        ])
        .assert()
        .success();

    // Only the init baseline remains.
    let entries: Vec<PathBuf> = fs::read_dir(&migrations)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("00000000000000_init"));

    let init_sql = fs::read_to_string(&entries[0]).unwrap();
    assert!(init_sql.starts_with("-- SQUASH EPOCH "));
    assert!(init_sql.to_ascii_lowercase().contains("create table"));
    assert!(!init_sql.contains("_sqlx_migrations"));

    // DB still has old history until migrate run.
    let mut conn = SqliteConnection::connect(&db_url).await.unwrap();
    assert_eq!(conn.list_applied_migrations().await.unwrap().len(), 2);

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args([
            "sqlx",
            "migrate",
            "run",
            "--database-url",
            &db_url,
            "--source",
            migrations.to_str().unwrap(),
        ])
        .assert()
        .success();

    let applied = conn.list_applied_migrations().await.unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].version, 0);

    // Data preserved (rewrite skipped schema SQL).
    let name: String = sqlx::query_scalar("SELECT name FROM items WHERE id = 1")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(name, "a");
}

#[tokio::test]
async fn squash_dry_run_leaves_files() {
    let tmp = tempdir().unwrap();
    let migrations = tmp.path().join("migrations");
    write_migrations(&migrations);

    let db_path = tmp.path().join("dry.db");
    let db_url = format!("sqlite://{}", db_path.display());

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args(["sqlx", "database", "create", "--database-url", &db_url])
        .assert()
        .success();

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args([
            "sqlx",
            "migrate",
            "run",
            "--database-url",
            &db_url,
            "--source",
            migrations.to_str().unwrap(),
        ])
        .assert()
        .success();

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args([
            "sqlx",
            "migrate",
            "squash",
            "--database-url",
            &db_url,
            "--source",
            migrations.to_str().unwrap(),
            "--dry-run",
            "--no-commit",
        ])
        .assert()
        .success();

    assert_eq!(fs::read_dir(&migrations).unwrap().count(), 2);
}

#[tokio::test]
async fn squash_fails_when_pending() {
    let tmp = tempdir().unwrap();
    let migrations = tmp.path().join("migrations");
    write_migrations(&migrations);

    let db_path = tmp.path().join("pending.db");
    let db_url = format!("sqlite://{}", db_path.display());

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args(["sqlx", "database", "create", "--database-url", &db_url])
        .assert()
        .success();

    // Apply only first migration by temporarily removing the second.
    let second = migrations.join("20220102000000_add_col.sql");
    let second_sql = fs::read_to_string(&second).unwrap();
    fs::remove_file(&second).unwrap();

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args([
            "sqlx",
            "migrate",
            "run",
            "--database-url",
            &db_url,
            "--source",
            migrations.to_str().unwrap(),
        ])
        .assert()
        .success();

    fs::write(&second, second_sql).unwrap();

    Command::cargo_bin("cargo-sqlx")
        .unwrap()
        .args([
            "sqlx",
            "migrate",
            "squash",
            "--database-url",
            &db_url,
            "--source",
            migrations.to_str().unwrap(),
            "--no-commit",
        ])
        .assert()
        .failure();
}
