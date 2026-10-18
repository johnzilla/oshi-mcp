//! Audit trail for gated calls (the ones that spend sats, change campaigns or
//! mint portal links): one JSON line on stderr per call, whatever the outcome.
//!
//! stdout carries the MCP protocol, so stderr is the operator's channel; most
//! MCP clients save it to a log file. Lines look like:
//!
//! ```text
//! {"approval":"approved","audit":"oshi-mcp","campaign_id":"…","external_id":"action_456","outcome":"ok","sats":100,"tool":"grant_reward","ts":"2026-10-18T14:03:12.500Z",…}
//! ```
//!
//! Records hold identifiers, amounts and statuses, never the API key, metadata
//! or portal links.

use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value};

use crate::redact;

const MAX_ERROR_CHARS: usize = 500;

/// Where audit lines go.
#[derive(Clone, Default)]
pub enum AuditSink {
    #[default]
    Stderr,
    /// Collects lines in memory, for tests.
    Memory(Arc<Mutex<Vec<String>>>),
}

impl AuditSink {
    pub fn memory() -> (Self, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        (Self::Memory(lines.clone()), lines)
    }

    fn write(&self, line: &str) {
        match self {
            // Losing an audit line must not fail the call, which has already happened.
            Self::Stderr => drop(writeln!(std::io::stderr().lock(), "{line}")),
            Self::Memory(lines) => lines.lock().unwrap().push(line.to_owned()),
        }
    }
}

/// How far a call got, which decides its outcome if it fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Validating,
    Approving,
    Sending,
}

/// The record for one gated call. Fields are added as they become known, and
/// `finish` writes it.
pub struct Entry {
    tool: &'static str,
    stage: Stage,
    approval: &'static str,
    outcome_unknown: bool,
    fields: Map<String, Value>,
}

impl Entry {
    pub fn new(tool: &'static str) -> Self {
        Self {
            tool,
            stage: Stage::Validating,
            approval: "not_given",
            outcome_unknown: false,
            fields: Map::new(),
        }
    }

    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        let value = value.into();
        if !value.is_null() {
            self.fields.insert(key.to_owned(), value);
        }
    }

    /// Copies `source[from]` into the record as `key`, if present.
    pub fn copy(&mut self, key: &str, source: &Value, from: &str) {
        self.set(key, source.get(from).cloned().unwrap_or(Value::Null));
    }

    pub fn stage(&mut self, stage: Stage) {
        self.stage = stage;
    }

    /// `"approved"` by the user, or `"not_required"` when confirmation is off.
    pub fn approval(&mut self, approval: &'static str) {
        self.approval = approval;
    }

    pub fn outcome_unknown(&mut self) {
        self.outcome_unknown = true;
    }

    pub fn finish(self, sink: &AuditSink, result: &Result<String, String>) {
        self.finish_at(sink, result, SystemTime::now());
    }

    fn finish_at(mut self, sink: &AuditSink, result: &Result<String, String>, now: SystemTime) {
        let outcome = match (result, self.stage) {
            (Ok(_), _) => "ok",
            (Err(_), Stage::Validating) => "invalid_input",
            (Err(_), Stage::Approving) => "not_approved",
            (Err(_), Stage::Sending) if self.outcome_unknown => "outcome_unknown",
            (Err(_), Stage::Sending) => "failed",
        };
        if let Err(e) = result {
            let e: String = redact::text(e).chars().take(MAX_ERROR_CHARS).collect();
            self.fields.insert("error".into(), e.into());
        }
        let mut record = Map::new();
        record.insert("ts".into(), rfc3339(now).into());
        record.insert("audit".into(), "oshi-mcp".into());
        record.insert("tool".into(), self.tool.into());
        record.insert("outcome".into(), outcome.into());
        record.insert("approval".into(), self.approval.into());
        for (k, v) in self.fields {
            record.entry(k).or_insert(v);
        }
        // Portal links never belong in a log, whatever the tool settings.
        sink.write(&redact::value(Value::Object(record)).to_string());
    }
}

/// Formats a time as RFC 3339 in UTC with milliseconds.
fn rfc3339(t: SystemTime) -> String {
    let ms = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis()) as i64;
    let (days, ms_of_day) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let secs = ms_of_day / 1000;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        ms_of_day % 1000
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    #[test]
    fn formats_rfc3339_utc() {
        assert_eq!(rfc3339(at(0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(at(951_782_400_000)), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339(at(1_792_332_192_500)), "2026-10-18T14:03:12.500Z");
        assert_eq!(rfc3339(at(4_107_542_399_999)), "2100-02-28T23:59:59.999Z");
    }

    fn record(entry: Entry, result: Result<String, String>) -> Value {
        let (sink, lines) = AuditSink::memory();
        entry.finish_at(&sink, &result, at(1_792_332_192_500));
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].contains('\n'), "one record per line");
        serde_json::from_str(&lines[0]).unwrap()
    }

    #[test]
    fn outcome_follows_the_stage_reached() {
        let cases = [
            (
                Stage::Validating,
                false,
                Err("bad".to_owned()),
                "invalid_input",
            ),
            (
                Stage::Approving,
                false,
                Err("declined".to_owned()),
                "not_approved",
            ),
            (Stage::Sending, false, Err("422".to_owned()), "failed"),
            (
                Stage::Sending,
                true,
                Err("timeout".to_owned()),
                "outcome_unknown",
            ),
            (Stage::Sending, false, Ok("{}".to_owned()), "ok"),
        ];
        for (stage, unknown, result, expected) in cases {
            let mut e = Entry::new("grant_reward");
            e.stage(stage);
            if unknown {
                e.outcome_unknown();
            }
            let failed = result.is_err();
            let r = record(e, result);
            assert_eq!(r["outcome"], expected);
            assert_eq!(r["error"].is_string(), failed, "{expected}");
        }
    }

    #[test]
    fn record_has_identifiers_and_never_portal_links() {
        let mut e = Entry::new("grant_reward");
        e.approval("approved");
        e.set("user_id", "user_123");
        e.set("lightning_address", Value::Null); // omitted, not logged as null
        let response = json!({"reward_event_id": "7f0a", "status": "pending",
            "portal_url": "https://portal.bitcoinrewards.app/a/Qm9ndXNUb2tlbkV4YW1wbGU"});
        e.copy("reward_event_id", &response, "reward_event_id");
        e.copy("status", &response, "status");
        e.copy("missing", &response, "nope");
        e.set(
            "note",
            "see https://portal.bitcoinrewards.app/a/Qm9ndXNUb2tlbkV4YW1wbGU",
        );
        e.stage(Stage::Sending);
        let r = record(e, Ok(String::new()));
        assert_eq!(r["ts"], "2026-10-18T14:03:12.500Z");
        assert_eq!(r["audit"], "oshi-mcp");
        assert_eq!(r["tool"], "grant_reward");
        assert_eq!(r["approval"], "approved");
        assert_eq!(r["user_id"], "user_123");
        assert_eq!(r["reward_event_id"], "7f0a");
        assert!(r.get("lightning_address").is_none() && r.get("missing").is_none());
        assert!(!r.to_string().contains("Qm9ndXNUb2tlbkV4YW1wbGU"), "{r}");
    }

    #[test]
    fn fields_cannot_overwrite_the_header() {
        let mut e = Entry::new("void_reward");
        e.set("outcome", "ok");
        e.set("tool", "grant_reward");
        let r = record(e, Err("declined".into()));
        assert_eq!(r["tool"], "void_reward");
        assert_eq!(r["outcome"], "invalid_input");
    }
}
