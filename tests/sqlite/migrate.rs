use sqlx::migrate::{
    format_squash_epoch_header, AppliedMigration, Migrate, MigrateError, Migration, MigrationType,
    Migrator,
};
use sqlx::pool::PoolConnection;
use sqlx::sqlite::{Sqlite, SqliteConnection};
use sqlx::Executor;
use sqlx::Row;
use std::borrow::Cow;
use std::path::Path;

#[sqlx::test(migrations = false)]
async fn simple(mut conn: PoolConnection<Sqlite>) -> anyhow::Result<()> {
    clean_up(&mut conn).await?;

    let migrator = Migrator::new(Path::new("tests/sqlite/migrations_simple")).await?;

    // run migration
    migrator.run(&mut conn).await?;

    // check outcome
    let res: String = conn
        .fetch_one("SELECT some_payload FROM migrations_simple_test")
        .await?
        .get(0);
    assert_eq!(res, "110_suffix");

    // running it a 2nd time should still work
    migrator.run(&mut conn).await?;

    Ok(())
}

#[sqlx::test(migrations = false)]
async fn squash_rewrite_and_new_db(mut conn: PoolConnection<Sqlite>) -> anyhow::Result<()> {
    clean_up(&mut conn).await?;

    // Apply original migrations.
    let original = Migrator::new(Path::new("tests/sqlite/migrations_simple")).await?;
    original.run(&mut conn).await?;

    let applied_before = conn.list_applied_migrations().await?;
    assert_eq!(applied_before.len(), 2);

    let epoch = format_squash_epoch_header(
        &applied_before
            .iter()
            .map(|m| m.checksum.as_ref())
            .collect::<Vec<_>>(),
    );

    // Build a squashed migrator in memory (same shape as the CLI would write).
    let dump = "CREATE TABLE migrations_simple_test (\n    some_id BIGINT NOT NULL PRIMARY KEY,\n    some_payload TEXT NOT NULL\n);";
    let init_sql = format!("{epoch}\n{dump}\n");
    let init = Migration::new(
        0,
        Cow::Borrowed("init"),
        MigrationType::Simple,
        Cow::Owned(init_sql),
        false,
    );

    let squashed = Migrator {
        migrations: Cow::Owned(vec![init.clone()]),
        ..Migrator::DEFAULT
    };

    // Existing DB: rewrite history, do not re-run schema SQL.
    squashed.run(&mut conn).await?;

    let applied_after: Vec<AppliedMigration> = conn.list_applied_migrations().await?;
    assert_eq!(applied_after.len(), 1);
    assert_eq!(applied_after[0].version, 0);
    assert_eq!(applied_after[0].checksum, init.checksum);

    // Table still has data from original migrations.
    let res: String = conn
        .fetch_one("SELECT some_payload FROM migrations_simple_test")
        .await?
        .get(0);
    assert_eq!(res, "110_suffix");

    // Idempotent.
    squashed.run(&mut conn).await?;
    assert_eq!(conn.list_applied_migrations().await?.len(), 1);

    // New DB: empty history applies baseline SQL.
    conn.execute("DROP TABLE migrations_simple_test").await.ok();
    conn.execute("DROP TABLE _sqlx_migrations").await.ok();

    squashed.run(&mut conn).await?;
    assert_eq!(conn.list_applied_migrations().await?.len(), 1);
    // Table exists (empty) from dump.
    let _: i64 = conn
        .fetch_one("SELECT COUNT(*) FROM migrations_simple_test")
        .await?
        .get(0);

    Ok(())
}

#[sqlx::test(migrations = false)]
async fn squash_epoch_mismatch(mut conn: PoolConnection<Sqlite>) -> anyhow::Result<()> {
    clean_up(&mut conn).await?;

    let original = Migrator::new(Path::new("tests/sqlite/migrations_simple")).await?;
    original.run(&mut conn).await?;

    let bad_epoch = format_squash_epoch_header(&[vec![0u8; 48], vec![1u8; 48]]);
    let init_sql = format!("{bad_epoch}\nCREATE TABLE t (id INT);\n");
    let init = Migration::new(
        0,
        Cow::Borrowed("init"),
        MigrationType::Simple,
        Cow::Owned(init_sql),
        false,
    );
    let squashed = Migrator {
        migrations: Cow::Owned(vec![init]),
        ..Migrator::DEFAULT
    };

    let err = squashed.run(&mut conn).await.unwrap_err();
    assert!(matches!(err, MigrateError::SquashEpochMismatch(0)));

    // History unchanged.
    assert_eq!(conn.list_applied_migrations().await?.len(), 2);

    Ok(())
}

#[sqlx::test(migrations = false)]
async fn squash_then_apply_later_migration(mut conn: PoolConnection<Sqlite>) -> anyhow::Result<()> {
    clean_up(&mut conn).await?;

    let original = Migrator::new(Path::new("tests/sqlite/migrations_simple")).await?;
    original.run(&mut conn).await?;
    let applied_before = conn.list_applied_migrations().await?;
    let epoch = format_squash_epoch_header(
        &applied_before
            .iter()
            .map(|m| m.checksum.as_ref())
            .collect::<Vec<_>>(),
    );

    let init_sql = format!(
        "{epoch}\nCREATE TABLE migrations_simple_test (\n    some_id BIGINT NOT NULL PRIMARY KEY,\n    some_payload TEXT NOT NULL\n);\n"
    );
    let init = Migration::new(
        0,
        Cow::Borrowed("init"),
        MigrationType::Simple,
        Cow::Owned(init_sql),
        false,
    );
    let extra = Migration::new(
        1,
        Cow::Borrowed("add col"),
        MigrationType::Simple,
        Cow::Borrowed("ALTER TABLE migrations_simple_test ADD COLUMN extra INT DEFAULT 0;"),
        false,
    );

    let migrator = Migrator {
        migrations: Cow::Owned(vec![init, extra]),
        ..Migrator::DEFAULT
    };

    // DB still has pre-squash history; should rewrite then apply version 1.
    migrator.run(&mut conn).await?;

    let applied = conn.list_applied_migrations().await?;
    assert_eq!(
        applied.iter().map(|m| m.version).collect::<Vec<_>>(),
        vec![0, 1]
    );

    let _: i64 = conn
        .fetch_one("SELECT extra FROM migrations_simple_test LIMIT 1")
        .await?
        .get(0);

    Ok(())
}

#[sqlx::test(migrations = false)]
async fn reversible(mut conn: PoolConnection<Sqlite>) -> anyhow::Result<()> {
    clean_up(&mut conn).await?;

    let migrator = Migrator::new(Path::new("tests/sqlite/migrations_reversible")).await?;

    // run migration
    migrator.run(&mut conn).await?;

    // check outcome
    let res: i64 = conn
        .fetch_one("SELECT some_payload FROM migrations_reversible_test")
        .await?
        .get(0);
    assert_eq!(res, 101);

    // roll back nothing (last version)
    migrator.undo(&mut conn, 20220721125033).await?;

    // check outcome
    let res: i64 = conn
        .fetch_one("SELECT some_payload FROM migrations_reversible_test")
        .await?
        .get(0);
    assert_eq!(res, 101);

    // roll back one version
    migrator.undo(&mut conn, 20220721124650).await?;

    // check outcome
    let res: i64 = conn
        .fetch_one("SELECT some_payload FROM migrations_reversible_test")
        .await?
        .get(0);
    assert_eq!(res, 100);

    Ok(())
}

/// Ensure that we have a clean initial state.
async fn clean_up(conn: &mut SqliteConnection) -> anyhow::Result<()> {
    conn.execute("DROP TABLE migrations_simple_test").await.ok();
    conn.execute("DROP TABLE migrations_reversible_test")
        .await
        .ok();
    conn.execute("DROP TABLE _sqlx_migrations").await.ok();

    Ok(())
}
