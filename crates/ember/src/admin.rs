//! Local installation and recovery commands. Passwords travel only through standard input.

use std::io::{IsTerminal, Read, Write};

use anyhow::{Context, bail};
use ember_db::{Account, Connection, PasswordDigest, User};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use rails_compat::encoding;

use crate::config::Config;

const MAX_PASSWORD_BYTES: usize = 4096;

/// Emit shell-safe environment assignments before an install has any configuration.
pub fn generate_secrets(mut output: impl Write) -> anyhow::Result<()> {
    let vapid = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    writeln!(output, "SECRET_KEY_BASE={}", hex::encode(rand::random::<[u8; 64]>()))?;
    writeln!(output, "VAPID_PUBLIC_KEY={}", encoding::urlsafe_encode_unpadded(vapid.public_key().to_encoded_point(false).as_bytes()))?;
    writeln!(output, "VAPID_PRIVATE_KEY={}", encoding::urlsafe_encode_unpadded(&vapid.to_bytes()))?;
    writeln!(output, "EMBER_SETUP_TOKEN={}", hex::encode(rand::random::<[u8; 32]>()))?;
    Ok(())
}

/// Report whether a first account exists without booting the application or creating a database.
pub fn setup_status(config: &Config, mut output: impl Write) -> anyhow::Result<()> {
    let conn = Connection::open_with_flags(&config.storage.database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("could not open the existing Ember database")?;
    let status = if Account::count(&conn)? == 0 { "pending" } else { "initialized" };
    writeln!(output, "{status}")?;
    Ok(())
}

pub fn reset_password(config: &Config, email: &str) -> anyhow::Result<()> {
    let input = std::io::stdin();
    if input.is_terminal() {
        bail!("Read the new password with a hidden prompt, then pipe it to ember reset-password; passwords are accepted only on stdin");
    }
    let password = read_password(input.lock())?;
    // Recovery must never silently create a new database at a mistaken storage path.
    let mut conn = Connection::open_with_flags(&config.storage.database, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .context("could not open the existing Ember database")?;
    ember_db::schema::configure_connection(&conn)?;
    let pending = ember_db::schema::pending_migrations(&conn)?;
    if !pending.is_empty() {
        bail!("database has pending migrations; migrate it before resetting a password");
    }
    reset_in_database(&mut conn, email, &password, rails_compat::password::COST)?;
    println!("Password reset; existing sessions revoked. Restart Ember to close existing WebSocket connections.");
    Ok(())
}

fn read_password(input: impl Read) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    input.take((MAX_PASSWORD_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_PASSWORD_BYTES {
        bail!("password input exceeds {MAX_PASSWORD_BYTES} bytes");
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    if bytes.iter().any(|byte| matches!(byte, b'\n' | b'\r' | 0)) {
        bail!("provide exactly one password on stdin");
    }
    let password = String::from_utf8(bytes).context("password must be UTF-8")?;
    if password.chars().count() < 8 {
        bail!("password must contain at least 8 characters");
    }
    Ok(password)
}

fn reset_in_database(conn: &mut Connection, email: &str, password: &str, cost: u32) -> anyhow::Result<()> {
    let digest = PasswordDigest::create(password, cost)?.into_string();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let user = User::find_by_email_address(&tx, email)?.context("no user has that email address")?;
    if user.is_bot() {
        bail!("bots cannot sign in with a password");
    }
    // This recovery operation changes credentials, never the account's role or active status.
    tx.execute(
        "UPDATE users SET password_digest = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![digest, ember_db::Timestamp::from_jiff(jiff::Timestamp::now()), user.id],
    )?;
    tx.execute("DELETE FROM sessions WHERE user_id = ?1", [user.id])?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::SetupToken;
    use crate::integrations::web_push::VapidConfig;

    #[test]
    fn generated_environment_has_valid_independent_secrets() {
        let mut first = Vec::new();
        let mut second = Vec::new();
        generate_secrets(&mut first).unwrap();
        generate_secrets(&mut second).unwrap();
        assert_ne!(first, second);
        let text = String::from_utf8(first).unwrap();
        let values: BTreeMap<_, _> = text.lines().map(|line| line.split_once('=').unwrap()).collect();
        assert_eq!(values.len(), 4);
        assert_eq!(hex::decode(values["SECRET_KEY_BASE"]).unwrap().len(), 64);
        SetupToken::parse(values["EMBER_SETUP_TOKEN"]).unwrap();
        VapidConfig::new("https://chat.example.test", values["VAPID_PUBLIC_KEY"], values["VAPID_PRIVATE_KEY"]).unwrap();
        assert!(values.values().all(|value| value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))));
    }

    #[test]
    fn setup_status_requires_an_existing_database_and_reports_account_presence() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::from_lookup(|name| match name {
            "SECRET_KEY_BASE" => Some("admin-status-test-secret".repeat(4)),
            "EMBER_STORAGE_PATH" => Some(dir.path().to_string_lossy().into_owned()),
            _ => None,
        })
        .unwrap();
        let mut output = Vec::new();
        assert!(setup_status(&config, &mut output).is_err());
        assert!(!config.storage.database.exists());
        assert!(output.is_empty());
        std::fs::create_dir_all(config.storage.database.parent().unwrap()).unwrap();
        let mut conn = Connection::open(&config.storage.database).unwrap();
        ember_db::schema::prepare(&mut conn, "test", &rails_compat::clock::SystemClock).unwrap();
        setup_status(&config, &mut output).unwrap();
        assert_eq!(output, b"pending\n");
        conn.execute_batch("INSERT INTO accounts (name, join_code, created_at, updated_at) VALUES ('Ember', 'a-b-c', '2026-01-01 00:00:00', '2026-01-01 00:00:00');")
            .unwrap();
        output.clear();
        setup_status(&config, &mut output).unwrap();
        assert_eq!(output, b"initialized\n");
    }

    #[test]
    fn password_input_is_bounded_and_preserves_intentional_spaces() {
        assert_eq!(read_password("  correct horse  \r\n".as_bytes()).unwrap(), "  correct horse  ");
        assert_eq!(read_password("éééééééé".as_bytes()).unwrap(), "éééééééé");
        for bad in [b"".as_slice(), b"short\n", b"one password\nextra", b"null\0password", "éééé".as_bytes()] {
            assert!(read_password(bad).is_err());
        }
        assert!(read_password(vec![b'x'; MAX_PASSWORD_BYTES + 1].as_slice()).is_err());
    }

    #[test]
    fn reset_revokes_only_the_targets_sessions_and_keeps_account_status() {
        let mut conn = Connection::open_in_memory().unwrap();
        ember_db::schema::prepare(&mut conn, "test", &rails_compat::clock::SystemClock).unwrap();
        conn.execute_batch(
            "INSERT INTO users (id, name, email_address, role, status, created_at, updated_at) VALUES
              (1, 'Ada', 'ada@example.test', 1, 2, '2026-01-01 00:00:00', '2026-01-01 00:00:00'),
              (2, 'Other', 'other@example.test', 0, 0, '2026-01-01 00:00:00', '2026-01-01 00:00:00'),
              (3, 'Bot', 'bot@example.test', 2, 0, '2026-01-01 00:00:00', '2026-01-01 00:00:00');
             INSERT INTO sessions (user_id, token, last_active_at, created_at, updated_at) VALUES
              (1, 'target', '2026-01-01 00:00:00', '2026-01-01 00:00:00', '2026-01-01 00:00:00'),
              (2, 'other', '2026-01-01 00:00:00', '2026-01-01 00:00:00', '2026-01-01 00:00:00');",
        )
        .unwrap();
        reset_in_database(&mut conn, "ada@example.test", "correct horse", rails_compat::password::MIN_COST).unwrap();
        let user = User::find_by_email_address(&conn, "ada@example.test").unwrap().unwrap();
        assert!(user.authenticate("correct horse"));
        assert_eq!(user.status, ember_db::Status::Banned);
        assert!(user.is_administrator());
        assert_eq!(ember_db::Session::count_for_user(&conn, 1).unwrap(), 0);
        assert_eq!(ember_db::Session::count_for_user(&conn, 2).unwrap(), 1);
        for email in ["bot@example.test", "missing@example.test"] {
            assert!(reset_in_database(&mut conn, email, "correct horse", rails_compat::password::MIN_COST).is_err());
        }
        assert!(User::find_by_email_address(&conn, "bot@example.test").unwrap().unwrap().password_digest.is_none());
    }
}
