//! The confirmation of destructive requests (retag, the Trash, removing
//! from, renaming or deleting a playlist): a request without
//! `confirm_token` changes nothing and answers the exact plan with a
//! one-time token, valid [`TTL`] for the same token and exactly the same
//! arguments; the same request with it does the change. A token is gone
//! once presented, whether it fit or not.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::error::ApiError;

/// How long a plan's token is good for.
pub const TTL: Duration = Duration::from_secs(300);
/// Plans waiting at most (the oldest go first).
const KEPT: usize = 64;

struct Pending {
    token_id: String,
    args: String,
    until: Instant,
}

/// The plans waiting for their confirmation.
#[derive(Default)]
pub struct Store {
    pending: Mutex<HashMap<String, Pending>>,
}

/// Why a confirmation was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Never given, used already, or of another token.
    Unknown,
    Expired,
    /// Given for other arguments.
    Mismatch,
}

impl From<Refused> for ApiError {
    fn from(r: Refused) -> Self {
        match r {
            Refused::Unknown => ApiError::of(StatusCode::CONFLICT, "confirm_unknown", "Токен подтверждения не найден или уже использован: запроси план заново"),
            Refused::Expired => ApiError::of(StatusCode::CONFLICT, "confirm_expired", "Токен подтверждения истёк: запроси план заново"),
            Refused::Mismatch => ApiError::of(StatusCode::CONFLICT, "confirm_mismatch", "Токен подтверждения выдан для других аргументов: запроси план заново"),
        }
    }
}

/// `v` with the keys of every object sorted, whatever order they came in.
fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canonical(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// What a plan is bound to: the route and its arguments (without the
/// token itself), as a hash.
pub fn args_hash(route: &str, args: &Value) -> String {
    let mut args = canonical(args);
    if let Some(m) = args.as_object_mut() {
        m.remove("confirm_token");
    }
    let text = format!("{route}\n{}", serde_json::to_string(&args).unwrap_or_default());
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

impl Store {
    /// A new token for the plan of `token_id` with `args` (a hash).
    pub fn issue(&self, token_id: &str, args: &str, now: Instant) -> Result<String, ApiError> {
        let secret = nikgob_core::api_token::generate().ok_or_else(|| ApiError::internal("Нет случайных чисел"))?;
        let token = format!("ct_{}", secret.trim_start_matches("nk_"));
        let mut p = self.pending.lock();
        p.retain(|_, x| x.until > now);
        while p.len() >= KEPT {
            let Some(oldest) = p.iter().min_by_key(|(_, x)| x.until).map(|(k, _)| k.clone()) else { break };
            p.remove(&oldest);
        }
        p.insert(token.clone(), Pending { token_id: token_id.to_owned(), args: args.to_owned(), until: now + TTL });
        Ok(token)
    }

    /// Takes `token` for `token_id` and `args`: Ok once, when it fits.
    pub fn take(&self, token: &str, token_id: &str, args: &str, now: Instant) -> Result<(), Refused> {
        let Some(p) = self.pending.lock().remove(token.trim()) else { return Err(Refused::Unknown) };
        if p.token_id != token_id {
            return Err(Refused::Unknown);
        }
        if p.until <= now {
            return Err(Refused::Expired);
        }
        if p.args != args {
            return Err(Refused::Mismatch);
        }
        Ok(())
    }
}

/// The answer of a request that needs a confirmation: what it would do.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PlanAnswer {
    /// `needs_confirmation`.
    pub status: String,
    /// What it is: `retag`, `trash`, `playlist_remove`, `playlist_rename`,
    /// `playlist_delete`.
    pub action: String,
    /// How many things it would change.
    pub affected: usize,
    /// Exactly what it would do (names, before → after).
    #[schema(value_type = Object)]
    pub plan: Value,
    /// Can the app undo it (the Trash, «Отменить»)?
    pub reversible: bool,
    /// Repeat the same request with this to do it.
    pub confirm_token: String,
    pub expires_in_s: u64,
}

impl PlanAnswer {
    pub fn new(action: &str, affected: usize, plan: Value, reversible: bool, confirm_token: String) -> Self {
        Self {
            status: "needs_confirmation".to_owned(),
            action: action.to_owned(),
            affected,
            plan,
            reversible,
            confirm_token,
            expires_in_s: TTL.as_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_token_works_once_for_its_arguments() {
        let s = Store::default();
        let t0 = Instant::now();
        let h = args_hash("POST /v1/library/trash", &json!({"track_ids": ["a"]}));
        let tok = s.issue("tok1", &h, t0).unwrap();
        assert!(tok.starts_with("ct_"));
        assert_eq!(s.take(&tok, "tok1", &h, t0 + Duration::from_secs(10)), Ok(()));
        assert_eq!(s.take(&tok, "tok1", &h, t0 + Duration::from_secs(11)), Err(Refused::Unknown), "reuse refused");
    }

    #[test]
    fn a_token_expires_and_binds_to_its_arguments_and_caller() {
        let s = Store::default();
        let t0 = Instant::now();
        let h = args_hash("POST /v1/library/trash", &json!({"track_ids": ["a"]}));
        let other = args_hash("POST /v1/library/trash", &json!({"track_ids": ["b"]}));
        let tok = s.issue("tok1", &h, t0).unwrap();
        assert_eq!(s.take(&tok, "tok1", &h, t0 + TTL + Duration::from_secs(1)), Err(Refused::Expired));
        let tok = s.issue("tok1", &h, t0).unwrap();
        assert_eq!(s.take(&tok, "tok1", &other, t0), Err(Refused::Mismatch));
        assert_eq!(s.take(&tok, "tok1", &h, t0), Err(Refused::Unknown), "gone after a wrong try");
        let tok = s.issue("tok1", &h, t0).unwrap();
        assert_eq!(s.take(&tok, "tok2", &h, t0), Err(Refused::Unknown), "another token's plan");
        assert_eq!(s.take("ct_nothing", "tok1", &h, t0), Err(Refused::Unknown));
        let e: ApiError = Refused::Expired.into();
        assert_eq!((e.status, e.code), (StatusCode::CONFLICT, "confirm_expired"));
    }

    #[test]
    fn the_hash_ignores_key_order_and_the_token() {
        let a = args_hash("r", &json!({"b": 1, "a": {"y": 2, "x": [1, 2]}}));
        let b = args_hash("r", &json!({"a": {"x": [1, 2], "y": 2}, "b": 1, "confirm_token": "ct_x"}));
        assert_eq!(a, b);
        assert_ne!(a, args_hash("r2", &json!({"b": 1, "a": {"y": 2, "x": [1, 2]}})));
        assert_ne!(a, args_hash("r", &json!({"b": 1, "a": {"y": 2, "x": [2, 1]}})));
    }

    #[test]
    fn the_store_keeps_a_bounded_number() {
        let s = Store::default();
        let t0 = Instant::now();
        let first = s.issue("t", "h", t0).unwrap();
        for i in 1..=KEPT as u64 {
            s.issue("t", "h", t0 + Duration::from_millis(i)).unwrap();
        }
        assert!(s.pending.lock().len() <= KEPT);
        assert_eq!(s.take(&first, "t", "h", t0), Err(Refused::Unknown), "the oldest went");
    }
}
