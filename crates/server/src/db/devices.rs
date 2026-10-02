//! Device and pairing-code persistence.

use rusqlite::OptionalExtension;
use rusqlite::TransactionBehavior;
use subtle::ConstantTimeEq;

use crate::db::Db;
use crate::db::models::Device;
use crate::error::{Result, ServerError};

impl Db {
    /// Fetches a device by id.
    pub fn device_by_id(&self, id: &str) -> Result<Option<Device>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, public_key, paired_at, last_seen, is_revoked, is_admin
             FROM devices WHERE id = ?1",
        )?;
        let device = stmt.query_row([id], row_to_device).optional()?;
        Ok(device)
    }

    /// Lists every paired device, newest first.
    pub fn list_devices(&self) -> Result<Vec<Device>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, public_key, paired_at, last_seen, is_revoked, is_admin
             FROM devices ORDER BY paired_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_device)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Revokes a device and forgets its playback progress. Returns `true`
    /// when a row was affected.
    pub fn revoke_device(&self, id: &str) -> Result<bool> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let affected = tx.execute(
            "UPDATE devices SET is_revoked = 1 WHERE id = ?1 AND is_revoked = 0",
            [id],
        )?;
        tx.execute("DELETE FROM progress WHERE device_id = ?1", [id])?;
        tx.commit()?;
        Ok(affected > 0)
    }

    /// Records that a device authenticated successfully at `now`.
    pub fn touch_device(&self, id: &str, now: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE devices SET last_seen = ?2 WHERE id = ?1",
            rusqlite::params![id, now],
        )?;
        Ok(())
    }

    /// Stores the hash of a freshly generated pairing code. A device paired
    /// with a code minted with `grants_admin` becomes an administrator.
    pub fn insert_pairing_code(
        &self,
        code_hash: &str,
        created_at: i64,
        expires_at: i64,
        grants_admin: bool,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO pairing_codes (code_hash, created_at, expires_at, used, grants_admin)
             VALUES (?1, ?2, ?3, 0, ?4)",
            rusqlite::params![code_hash, created_at, expires_at, grants_admin as i64],
        )?;
        Ok(())
    }

    /// Consumes a valid pairing code, comparing hashes in constant time.
    ///
    /// Returns `false` for unknown, expired or already-used codes without
    /// revealing which of those applies.
    ///
    /// Runs in an `IMMEDIATE` transaction so two concurrent requests cannot
    /// both consume the same code: the second waits for the first's write lock
    /// and then re-reads the row, which is already `used`.
    pub fn consume_pairing_code(&self, code_hash: &str, now: i64) -> Result<bool> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let consumed = consume_code_tx(&tx, code_hash, now)?;
        tx.commit()?;
        Ok(consumed.is_some())
    }

    /// Atomically consumes a pairing code and registers the device in one
    /// transaction, so a code can never be burned without creating its device.
    ///
    /// The device's `is_admin` flag is taken from the code, never from the
    /// caller. Returns the registered device, or `None` for an invalid code.
    pub fn pair_device_with_code(
        &self,
        code_hash: &str,
        now: i64,
        device: &Device,
    ) -> Result<Option<Device>> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(grants_admin) = consume_code_tx(&tx, code_hash, now)? else {
            tx.commit()?;
            return Ok(None);
        };
        let device = Device {
            is_admin: grants_admin,
            ..device.clone()
        };
        insert_device_conn(&tx, &device)?;
        tx.commit()?;
        Ok(Some(device))
    }

    /// Removes pairing codes whose expiry has passed.
    pub fn purge_expired_pairing_codes(&self, now: i64) -> Result<usize> {
        let conn = self.conn()?;
        let removed = conn.execute("DELETE FROM pairing_codes WHERE expires_at <= ?1", [now])?;
        Ok(removed)
    }
}

fn decode_hash(value: &str) -> Option<Vec<u8>> {
    hex::decode(value).ok()
}

fn insert_device_conn(conn: &rusqlite::Connection, device: &Device) -> Result<()> {
    conn.execute(
        "INSERT INTO devices (id, name, public_key, paired_at, last_seen, is_revoked, is_admin)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            device.id,
            device.name,
            device.public_key,
            device.paired_at,
            device.last_seen,
            device.is_revoked as i64,
            device.is_admin as i64,
        ],
    )?;
    Ok(())
}

/// Matches and marks a pairing code used. Must run inside a transaction.
///
/// Returns `Some(grants_admin)` when a code was consumed.
fn consume_code_tx(conn: &rusqlite::Connection, code_hash: &str, now: i64) -> Result<Option<bool>> {
    let expected = decode_hash(code_hash);
    let candidates = {
        let mut stmt = conn.prepare(
            "SELECT id, code_hash, grants_admin FROM pairing_codes
             WHERE used = 0 AND expires_at > ?1",
        )?;
        stmt.query_map([now], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut matched: Option<(i64, bool)> = None;
    for (id, stored, grants_admin) in candidates {
        if constant_time_eq(expected.as_deref(), decode_hash(&stored).as_deref()) {
            matched = Some((id, grants_admin));
        }
    }
    let Some((id, grants_admin)) = matched else {
        return Ok(None);
    };
    let updated = conn.execute(
        "UPDATE pairing_codes SET used = 1 WHERE id = ?1 AND used = 0",
        [id],
    )?;
    Ok((updated == 1).then_some(grants_admin))
}

fn constant_time_eq(a: Option<&[u8]>, b: Option<&[u8]>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) if a.len() == b.len() => a.ct_eq(b).into(),
        _ => false,
    }
}

fn row_to_device(row: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: row.get("id")?,
        name: row.get("name")?,
        public_key: row.get("public_key")?,
        paired_at: row.get("paired_at")?,
        last_seen: row.get("last_seen")?,
        is_revoked: row.get::<_, i64>("is_revoked")? != 0,
        is_admin: row.get::<_, i64>("is_admin")? != 0,
    })
}

/// Validates the shape of a generated code (exactly six ASCII digits).
pub fn is_valid_pairing_code_format(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit())
}

/// Converts a device's stored public key into a typed key, or a token error.
pub fn parse_device_public_key(
    public_key: &str,
) -> Result<pasetors::keys::AsymmetricPublicKey<pasetors::version4::V4>> {
    use std::convert::TryFrom;
    pasetors::keys::AsymmetricPublicKey::<pasetors::version4::V4>::try_from(public_key)
        .map_err(|_| ServerError::Token("device public key is not a valid PASERK key".into()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Barrier;

    use super::*;

    fn temp_db() -> Db {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "netvideo-devices-{}-{unique}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        Db::open_at(&path).expect("open db")
    }

    #[test]
    fn pairing_code_is_single_use() {
        let db = temp_db();
        let now = crate::util::unix_now();
        db.insert_pairing_code("aabb", now, now + 600, false)
            .unwrap();
        assert!(db.consume_pairing_code("aabb", now).unwrap());
        assert!(!db.consume_pairing_code("aabb", now).unwrap());
    }

    #[test]
    fn expired_pairing_code_cannot_be_consumed() {
        let db = temp_db();
        let now = crate::util::unix_now();
        db.insert_pairing_code("aabb", now - 700, now - 100, false)
            .unwrap();
        assert!(!db.consume_pairing_code("aabb", now).unwrap());
    }

    #[test]
    fn concurrent_consumers_win_at_most_once() {
        let db = Arc::new(temp_db());
        let now = crate::util::unix_now();
        db.insert_pairing_code("aabb", now, now + 600, false)
            .unwrap();

        let threads = 8;
        let barrier = Arc::new(Barrier::new(threads));
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let db = Arc::clone(&db);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    db.consume_pairing_code("aabb", now).unwrap()
                })
            })
            .collect();
        let mut wins = 0;
        for handle in handles {
            if handle.join().unwrap() {
                wins += 1;
            }
        }
        assert_eq!(wins, 1, "exactly one concurrent consumer may win");
    }

    #[test]
    fn wrong_code_does_not_consume() {
        let db = temp_db();
        let now = crate::util::unix_now();
        db.insert_pairing_code("aabb", now, now + 600, false)
            .unwrap();
        assert!(!db.consume_pairing_code("ccdd", now).unwrap());
        assert!(db.consume_pairing_code("aabb", now).unwrap());
    }

    fn sample_device(id: &str) -> Device {
        Device {
            id: id.into(),
            name: "phone".into(),
            public_key: "k4.public.x".into(),
            paired_at: 1,
            last_seen: None,
            is_revoked: false,
            is_admin: true,
        }
    }

    #[test]
    fn admin_flag_comes_from_the_code_not_the_caller() {
        let db = temp_db();
        let now = crate::util::unix_now();
        db.insert_pairing_code("aabb", now, now + 600, false)
            .unwrap();
        db.insert_pairing_code("ccdd", now, now + 600, true)
            .unwrap();

        let viewer = db
            .pair_device_with_code("aabb", now, &sample_device("v"))
            .unwrap()
            .expect("paired");
        assert!(!viewer.is_admin, "a viewer code must not grant admin");
        let admin = db
            .pair_device_with_code("ccdd", now, &sample_device("a"))
            .unwrap()
            .expect("paired");
        assert!(admin.is_admin);
        assert!(db.device_by_id("v").unwrap().is_some_and(|d| !d.is_admin));
        assert!(db.device_by_id("a").unwrap().is_some_and(|d| d.is_admin));
    }
}
