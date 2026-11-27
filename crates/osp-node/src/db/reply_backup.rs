//! What a backup keeps of replies (ADR 0028): replies to the owner's posts
//! and their answers, the policy of each post, who may reply without asking
//! or is blocked, replies the owner sent and where each stands, and the
//! deletes of replies, so a deleted one doesn't come back.
//!
//! Each table is kept as rows of column names and values, and a restore
//! writes only the columns the table has now: a backup made before a
//! migration added one restores with its default, and a column later
//! dropped is left out.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use iroh::PublicKey;
use osp_proto::SignedObject;
use rusqlite::{params_from_iter, types::ValueRef};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::Db;

/// The tables kept, in the order they're restored.
const TABLES: [&str; 5] = [
    "reply_policies",
    "repliers",
    "received_replies",
    "reply_deletes",
    "sent_replies",
];

/// The reply tables' rows, by table.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct ReplyTables(BTreeMap<String, Vec<Map<String, Value>>>);

impl Db {
    /// The reply tables, for a backup.
    pub(crate) fn reply_tables(&self) -> Result<ReplyTables> {
        let conn = self.conn();
        let mut out = BTreeMap::new();
        for table in TABLES {
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
            let names: Vec<String> = stmt.column_names().map(String::from).collect();
            let mut rows = stmt.query([])?;
            let mut kept = Vec::new();
            while let Some(row) = rows.next()? {
                let mut fields = Map::new();
                for (i, name) in names.iter().enumerate() {
                    let value = match row.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(n) => n.into(),
                        ValueRef::Real(x) => x.into(),
                        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
                        ValueRef::Blob(_) => bail!("{table}.{name} holds bytes"),
                    };
                    fields.insert(name.clone(), value);
                }
                kept.push(fields);
            }
            out.insert(table.to_string(), kept);
        }
        Ok(ReplyTables(out))
    }

    /// Restores the reply tables from a backup of `me`. A row whose signed
    /// reply or delete doesn't check out is left out. Returns how many
    /// replies came back, received and sent.
    pub(crate) fn restore_reply_tables(
        &self,
        tables: &ReplyTables,
        me: &PublicKey,
    ) -> Result<usize> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let mut replies = 0;
        for table in TABLES {
            let Some(rows) = tables.0.get(table) else {
                continue;
            };
            let columns: Vec<String> = tx
                .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for row in rows {
                if !signed_rightly(row, me) {
                    tracing::warn!(table, "skipping a row in the backup that doesn't check out");
                    continue;
                }
                let (names, values): (Vec<&String>, Vec<rusqlite::types::Value>) = row
                    .iter()
                    .filter(|(name, _)| columns.contains(name))
                    .map(|(name, value)| (name, to_sql(value)))
                    .unzip();
                if names.is_empty() {
                    continue;
                }
                let names: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
                let marks = vec!["?"; names.len()].join(", ");
                let sql = format!(
                    "INSERT OR IGNORE INTO {table} ({}) VALUES ({marks})",
                    names.join(", ")
                );
                // Say a column a later migration added without a default:
                // that row is lost, not the whole restore.
                let added = match tx.execute(&sql, params_from_iter(values)) {
                    Ok(added) => added,
                    Err(e) => {
                        tracing::warn!(table, "skipping a row in the backup: {e}");
                        continue;
                    }
                };
                if matches!(table, "received_replies" | "sent_replies") {
                    replies += added;
                }
            }
        }
        tx.commit()?;
        Ok(replies)
    }
}

/// Whether each signed object in `row` checks out and is signed by its
/// `actor`, or for a row without one, by the owner `me`.
fn signed_rightly(row: &Map<String, Value>, me: &PublicKey) -> bool {
    let signer = match row.get("actor") {
        Some(Value::String(actor)) => match actor.parse::<PublicKey>() {
            Ok(actor) => actor,
            Err(_) => return false,
        },
        Some(_) => return false,
        None => *me,
    };
    ["signed", "delete_signed"]
        .iter()
        .filter_map(|column| row.get(*column))
        .all(|value| match value {
            Value::Null => true,
            Value::String(json) => serde_json::from_str::<SignedObject>(json)
                .ok()
                .and_then(|obj| obj.verify().ok())
                .is_some_and(|payload| payload.actor() == signer),
            _ => false,
        })
}

fn to_sql(value: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as Sql;
    match value {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(*b as i64),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Sql::Integer(i),
            None => Sql::Real(n.as_f64().unwrap_or_default()),
        },
        Value::String(s) => Sql::Text(s.clone()),
        other => Sql::Text(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Note, Payload};
    use serde_json::json;

    use super::*;

    /// A reply by `key`, signed and written as the tables keep it.
    fn reply_by(key: &SecretKey) -> String {
        let payload = Payload::Note(Note {
            actor: key.public(),
            content: "nice".into(),
            published: "2026-11-27T00:00:00Z".into(),
            attachment: vec![],
            in_reply_to: None,
        });
        serde_json::to_string(&SignedObject::sign(key, &payload).unwrap()).unwrap()
    }

    #[test]
    fn a_restore_keeps_what_checks_out_in_the_columns_there_are() {
        let dir = tempfile::tempdir().unwrap();
        let (me, kim, jimmy) = (
            SecretKey::generate(),
            SecretKey::generate(),
            SecretKey::generate(),
        );
        let received = |id: &str, signed: String| {
            json!({
                "id": id, "post": "p", "actor": kim.public().to_string(), "node": "n",
                "state": "published", "thread_seq": null, "signed": signed,
                "profile": "{}", "received_ms": 1,
            })
        };
        let sent = |id: &str, signed: String| {
            json!({
                "id": id, "post": "q", "creator": "c", "creator_node": "n",
                "signed": signed, "delete_signed": null, "state": "sending",
                "error": null, "next_try_ms": 5, "since_ms": 1, "sent_ms": 1,
            })
        };
        let backup: ReplyTables = serde_json::from_value(json!({
            // A column since dropped is left out.
            "reply_policies": [{"post": "p", "policy": "circle", "dropped": 1}],
            // A column with no default missing: that row is lost.
            "repliers": [{"actor": kim.public().to_string(), "choice": "allow"}],
            // Kim's reply, and one Jimmy forged in her name.
            "received_replies": [received("r1", reply_by(&kim)), received("r2", reply_by(&jimmy))],
            // The owner's reply, with `tries` missing as from before it was
            // added, and one someone else signed.
            "sent_replies": [sent("s1", reply_by(&me)), sent("s2", reply_by(&kim))],
        }))
        .unwrap();

        let db = Db::open(&dir.path().join("restored.db")).unwrap();
        assert_eq!(db.restore_reply_tables(&backup, &me.public()).unwrap(), 2);
        let back = db.reply_tables().unwrap().0;
        assert_eq!(
            back["reply_policies"],
            [json!({"post": "p", "policy": "circle"})
                .as_object()
                .unwrap()
                .clone()]
        );
        assert!(back["repliers"].is_empty());
        let ids =
            |table: &str| -> Vec<Value> { back[table].iter().map(|r| r["id"].clone()).collect() };
        assert_eq!(ids("received_replies"), ["r1"]);
        assert_eq!(ids("sent_replies"), ["s1"]);
        assert_eq!(back["sent_replies"][0]["tries"], 0);

        // What a restored node backs up restores the same again.
        let again = Db::open(&dir.path().join("again.db")).unwrap();
        let tables = db.reply_tables().unwrap();
        again.restore_reply_tables(&tables, &me.public()).unwrap();
        assert_eq!(again.reply_tables().unwrap().0, tables.0);
    }
}
