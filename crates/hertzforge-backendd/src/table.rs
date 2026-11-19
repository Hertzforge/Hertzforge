//! THE DAEMON-AUTHORITATIVE CHANNEL TABLE, with versioned upload.
//!
//! # Why the daemon owns it
//!
//! In one process the channel table is a document the UI edits. Split across two
//! machines it is the thing the RUNNING ACQUISITION IS BOUND TO, and it lives where
//! the hardware lives - the `physical` column names devices that exist on the daemon's
//! host and nowhere else. A front end that treated its own copy as authoritative would
//! be able to arm a run against channels the daemon never agreed to.
//!
//! # The etag, and why a stale write is REFUSED rather than merged
//!
//! Every read hands back an etag; every write quotes the etag it was derived from; a
//! write quoting anything else is [`ProtocolError::StaleUpload`] naming both.
//!
//! Last-write-wins would be defensible for a document of preferences. This document
//! carries the ABORT LIMITS. Two operators (or one operator with a stale second
//! window) can otherwise each add a limit and have the second write silently delete
//! the first's - and nothing would say so until a run failed to abort. Refusing costs
//! a re-read; the alternative costs an armature.
//!
//! # The one permissive case, and its bound
//!
//! An EMPTY quoted etag is accepted only when the table has never been written in this
//! daemon's lifetime (`etag == ""` on both sides). That is the "a front end is
//! seeding a fresh daemon" case. It is NOT a general escape hatch: once anything has
//! been stored, a blank etag is as stale as a wrong one.

use std::sync::Mutex;

use hertzforge_channel_table::ChannelTable;
use hertzforge_protocol::{ProtocolError, TableDoc};

/// The daemon's single channel table.
#[derive(Debug)]
pub struct TableStore {
    inner: Mutex<Stored>,
}

#[derive(Debug)]
struct Stored {
    table: ChannelTable,
    etag: String,
}

impl TableStore {
    /// A store seeded with `table`.
    ///
    /// The seed's etag is computed from its content like any other, EXCEPT for the
    /// default-empty table, which is deliberately given the empty etag so a first
    /// write from a front end can seed it (see the module docs).
    pub fn new(table: ChannelTable) -> Self {
        let etag = if table.channels.is_empty() {
            String::new()
        } else {
            etag_of(&table)
        };
        TableStore {
            inner: Mutex::new(Stored { table, etag }),
        }
    }

    /// The current table, as a wire document.
    pub fn read(&self) -> TableDoc {
        let g = self.inner.lock().expect("table store poisoned");
        TableDoc::new(
            g.etag.clone(),
            g.table.to_json().unwrap_or_else(|_| "{}".to_string()),
        )
    }

    /// A clone of the live table, for building a station.
    pub fn snapshot(&self) -> ChannelTable {
        self.inner
            .lock()
            .expect("table store poisoned")
            .table
            .clone()
    }

    /// Store `doc`, enforcing the schema and the etag. Answers with the table AS
    /// STORED and its new etag.
    ///
    /// Answering with the stored document rather than a bare acknowledgement is
    /// deliberate: the client needs the fresh etag to make a second edit, and any
    /// core-side migration (`migrate_to_current`) becomes visible immediately instead
    /// of being discovered on the next read.
    pub fn write(&self, doc: &TableDoc) -> Result<TableDoc, ProtocolError> {
        doc.check()?;

        let mut incoming =
            ChannelTable::from_json(&doc.json).map_err(|e| ProtocolError::MalformedPayload {
                container: "channel table".into(),
                detail: e.to_string(),
            })?;
        incoming
            .migrate_to_current()
            .map_err(|e| ProtocolError::MalformedPayload {
                container: "channel table".into(),
                detail: e.to_string(),
            })?;
        // VALIDATED before it is stored. A table the daemon would refuse to arm on is
        // not a table worth storing, and finding out at arm puts the diagnosis three
        // screens from the edit.
        incoming
            .validate()
            .map_err(|e| ProtocolError::InvalidArgument {
                message: format!("the uploaded channel table is not valid: {e}"),
            })?;

        let mut g = self.inner.lock().expect("table store poisoned");
        if doc.etag != g.etag {
            return Err(ProtocolError::StaleUpload {
                current_etag: g.etag.clone(),
                provided_etag: doc.etag.clone(),
            });
        }

        let etag = etag_of(&incoming);
        g.table = incoming;
        g.etag = etag.clone();
        Ok(TableDoc::new(
            etag,
            g.table.to_json().unwrap_or_else(|_| "{}".to_string()),
        ))
    }
}

/// The etag: a 64-bit FNV-1a digest of the table's canonical JSON, hex-encoded.
///
/// Hand-rolled rather than pulled from a hashing crate, for the same reason
/// `hertzforge-protocol` hand-rolls its `Display` impls: the contract is only "changes when
/// the content changes, stable when it does not", and that does not justify a
/// dependency. It is NOT a security primitive and nothing authenticates with it - an
/// attacker who can write the table can also read its etag first.
pub fn etag_of(table: &ChannelTable) -> String {
    let json = table.to_json().unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in json.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_channel_table::{Channel, ChannelRole, PhysicalChannelRef};

    fn table(n: usize) -> ChannelTable {
        ChannelTable::default_with_channels(n)
    }

    /// THE STALE-UPLOAD REFUSAL, which is the whole point of the etag.
    ///
    /// RED-MUTATION: delete the `doc.etag != g.etag` check (i.e. last-write-wins) and
    /// this reds - which is the state where the second operator's write silently
    /// deletes the abort limit the first one just added.
    #[test]
    fn a_write_quoting_a_stale_etag_is_refused_and_names_both() {
        let store = TableStore::new(table(4));
        let first = store.read();
        assert!(!first.etag.is_empty());

        // Operator A edits and writes: accepted, and gets a NEW etag back.
        let mut edited = store.snapshot();
        edited.channels[1].role = ChannelRole::Drive;
        let a = TableDoc::new(first.etag.clone(), edited.to_json().unwrap());
        let after_a = store.write(&a).expect("A's write lands");
        assert_ne!(after_a.etag, first.etag, "the etag must move");

        // Operator B, holding the ORIGINAL etag, writes: REFUSED, both etags named.
        let mut b_edit = table(4);
        b_edit.channels[2].abort.accel_g = Some(9.0);
        let b = TableDoc::new(first.etag.clone(), b_edit.to_json().unwrap());
        match store.write(&b).unwrap_err() {
            ProtocolError::StaleUpload {
                current_etag,
                provided_etag,
            } => {
                assert_eq!(current_etag, after_a.etag);
                assert_eq!(provided_etag, first.etag);
            }
            other => panic!("expected StaleUpload, got {other:?}"),
        }

        // A's edit is intact - B did not clobber it.
        assert_eq!(store.snapshot().channels[1].role, ChannelRole::Drive);

        // Re-reading and re-applying works, which is the documented remedy.
        let fresh = store.read();
        let mut merged = store.snapshot();
        merged.channels[2].abort.accel_g = Some(9.0);
        let redo = TableDoc::new(fresh.etag, merged.to_json().unwrap());
        store.write(&redo).expect("the re-applied edit lands");
        assert_eq!(store.snapshot().channels[2].abort.accel_g, Some(9.0));
        assert_eq!(store.snapshot().channels[1].role, ChannelRole::Drive);
    }

    /// A blank etag seeds an EMPTY store and is stale forever after. The permissive
    /// case has a bound.
    #[test]
    fn a_blank_etag_seeds_an_empty_store_exactly_once() {
        let store = TableStore::new(ChannelTable::new());
        assert_eq!(store.read().etag, "", "an unseeded store has no etag");

        let seed = TableDoc::new("", table(3).to_json().unwrap());
        let after = store.write(&seed).expect("the seed lands");
        assert!(!after.etag.is_empty());

        // A SECOND blank-etag write is as stale as a wrong one.
        let again = TableDoc::new("", table(5).to_json().unwrap());
        assert!(matches!(
            store.write(&again).unwrap_err(),
            ProtocolError::StaleUpload { .. }
        ));
        assert_eq!(store.snapshot().channels.len(), 3, "unchanged");
    }

    /// The etag is content-derived: identical content is the same etag, one changed
    /// field is a different one. Without both halves it could be a counter that
    /// happens to work.
    #[test]
    fn the_etag_follows_the_content_in_both_directions() {
        let a = table(4);
        let b = table(4);
        assert_eq!(etag_of(&a), etag_of(&b), "identical tables, identical etag");

        let mut c = table(4);
        c.channels[0].physical = Some(PhysicalChannelRef::new("cDAQ1Mod1", "ai3"));
        assert_ne!(etag_of(&a), etag_of(&c), "a changed binding must move it");

        let mut d = table(4);
        d.channels.push(Channel::new(5));
        assert_ne!(etag_of(&a), etag_of(&d));
    }

    /// A document at a foreign SCHEMA is refused before it is parsed, and an
    /// unparseable one carries the decoder's own message.
    #[test]
    fn a_bad_document_is_refused_with_the_reason_the_operator_needs() {
        let store = TableStore::new(table(2));
        let etag = store.read().etag;

        let future = TableDoc {
            schema_version: 99,
            etag: etag.clone(),
            json: "{}".into(),
        };
        assert!(matches!(
            store.write(&future).unwrap_err(),
            ProtocolError::UnsupportedSchema { .. }
        ));

        let garbage = TableDoc::new(etag.clone(), "not json at all");
        match store.write(&garbage).unwrap_err() {
            ProtocolError::MalformedPayload { container, detail } => {
                assert_eq!(container, "channel table");
                assert!(!detail.is_empty(), "the decoder's message must survive");
            }
            other => panic!("expected MalformedPayload, got {other:?}"),
        }

        // ... and the store is untouched by either refusal.
        assert_eq!(store.read().etag, etag);
    }

    /// An INVALID table is refused at upload, not at arm. Finding out three screens
    /// later is the failure this check exists to prevent.
    #[test]
    fn an_invalid_table_is_refused_at_upload() {
        let store = TableStore::new(table(2));
        let etag = store.read().etag;
        let mut bad = table(2);
        // Duplicate indices: a table `validate()` rejects.
        bad.channels[1].index = bad.channels[0].index;
        let doc = TableDoc::new(etag.clone(), bad.to_json().unwrap());
        assert!(matches!(
            store.write(&doc).unwrap_err(),
            ProtocolError::InvalidArgument { .. }
        ));
        assert_eq!(store.read().etag, etag, "nothing was stored");
    }
}
