use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use manager_core::model::{User, UserId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use uuid::Uuid;

pub struct AuthStore(Connection);

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid username, password, or timestamp")]
    InvalidInput,
    #[error("administrator already initialized")]
    AlreadyInitialized,
    #[error("administrator access required")]
    Forbidden,
    #[error("invalid or expired invitation")]
    InvalidInvite,
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("storage operation failed")]
    Storage(#[from] rusqlite::Error),
    #[error("password processing failed")]
    Password,
}

impl AuthStore {
    pub fn open(path: &Path) -> Result<Self, AuthError> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(include_str!("../migrations/0002_auth.sql"))?;
        Ok(Self(connection))
    }

    pub fn bootstrap_admin(&mut self, username: &str, password: &str) -> Result<User, AuthError> {
        validate_credentials(username, password)?;
        let hash = hash_password(password)?;
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: i64 =
            transaction.query_row("SELECT count(*) FROM users", [], |row| row.get(0))?;
        if count != 0 {
            return Err(AuthError::AlreadyInitialized);
        }
        let user = User {
            id: UserId(Uuid::new_v4()),
            is_admin: true,
        };
        transaction.execute(
            "INSERT INTO users VALUES (?1, ?2, ?3, 1)",
            params![user.id.0.to_string(), username, hash],
        )?;
        transaction.commit()?;
        Ok(user)
    }

    pub fn invite(&mut self, actor: &User, now: i64) -> Result<String, AuthError> {
        let expires = expiry(now, 86400)?;
        let token = token();
        let changed = self.0.execute(
            "INSERT INTO invites (token_hash, creator_id, expires_at)
             SELECT ?1, id, ?2 FROM users WHERE id = ?3 AND is_admin = 1",
            params![token_hash(&token), expires, actor.id.0.to_string()],
        )?;
        if changed != 1 {
            return Err(AuthError::Forbidden);
        }
        Ok(token)
    }

    pub fn register(
        &mut self,
        invite: &str,
        username: &str,
        password: &str,
        now: i64,
    ) -> Result<User, AuthError> {
        validate_credentials(username, password)?;
        expiry(now, 1)?;
        let hash = hash_password(password)?;
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let consumed = transaction.execute(
            "UPDATE invites SET consumed_at = ?1 WHERE token_hash = ?2 AND consumed_at IS NULL AND expires_at > ?1",
            params![now, token_hash(invite)],
        )?;
        if consumed != 1 {
            return Err(AuthError::InvalidInvite);
        }
        let user = User {
            id: UserId(Uuid::new_v4()),
            is_admin: false,
        };
        transaction.execute(
            "INSERT INTO users VALUES (?1, ?2, ?3, 0)",
            params![user.id.0.to_string(), username, hash],
        )?;
        transaction.commit()?;
        Ok(user)
    }

    pub fn login(&mut self, username: &str, password: &str, now: i64) -> Result<String, AuthError> {
        let expires = expiry(now, 7 * 86400)?;
        if password.len() > 1024 {
            return Err(AuthError::InvalidCredentials);
        }
        let record: Option<(String, String)> = self
            .0
            .query_row(
                "SELECT id, password_hash FROM users WHERE username = ?1",
                [username],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((id, hash)) = record else {
            // Spend the password hash cost for unknown accounts too.
            hash_password(password)?;
            return Err(AuthError::InvalidCredentials);
        };
        let parsed = PasswordHash::new(&hash).map_err(|_| AuthError::Password)?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .map_err(|_| AuthError::InvalidCredentials)?;
        let token = token();
        self.0.execute(
            "INSERT INTO sessions VALUES (?1, ?2, ?3)",
            params![token_hash(&token), id, expires],
        )?;
        Ok(token)
    }

    pub fn authenticate(&self, token: &str, now: i64) -> Result<Option<User>, AuthError> {
        expiry(now, 1)?;
        let record: Option<(String, bool)> = self.0.query_row(
            "SELECT users.id, users.is_admin FROM sessions JOIN users ON users.id = sessions.user_id
             WHERE sessions.token_hash = ?1 AND sessions.expires_at > ?2",
            params![token_hash(token), now], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        record
            .map(|(id, is_admin)| {
                let id = Uuid::parse_str(&id).map_err(|_| AuthError::InvalidInput)?;
                Ok(User {
                    id: UserId(id),
                    is_admin,
                })
            })
            .transpose()
    }

    pub fn logout(&mut self, token: &str) -> Result<(), AuthError> {
        self.0.execute(
            "DELETE FROM sessions WHERE token_hash = ?1",
            [token_hash(token)],
        )?;
        Ok(())
    }
}

fn validate_credentials(username: &str, password: &str) -> Result<(), AuthError> {
    if !(3..=32).contains(&username.len())
        || !username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || !(12..=1024).contains(&password.len())
    {
        return Err(AuthError::InvalidInput);
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt =
        SaltString::encode_b64(Uuid::new_v4().as_bytes()).map_err(|_| AuthError::Password)?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AuthError::Password)
}

fn token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
fn expiry(now: i64, duration: i64) -> Result<i64, AuthError> {
    now.checked_add(duration)
        .filter(|_| now >= 0)
        .ok_or(AuthError::InvalidInput)
}
