//! Taking a document's outgoing delta from the writes themselves, rather than
//! reconstructing it afterwards.
//!
//! Emitting an edit used to mean asking the document what had changed since a
//! state vector taken just before it: `encode_diff_v1`. That answer is correct
//! but it is computed by walking the *whole* document — every client's blocks,
//! plus a `DeleteSet` built by scanning every block in the store — so the cost
//! of publishing a one-character edit grew with the size of the scheme it was
//! made in. On a 5,000-item scheme it was roughly a fifth of the entire
//! model-side keystroke.
//!
//! yrs already knows the answer for free. Every committed transaction carries
//! the bytes of exactly what it did, and `observe_update_v1` hands them over on
//! commit. Capturing those and merging them yields the same delta in time
//! proportional to the edit instead of the document.
//!
//! # The one semantic difference
//!
//! `encode_diff_v1` attaches the document's **entire** delete set to every
//! delta, so each push re-sends every tombstone the document has ever held.
//! Captured updates carry only the deletes their own transaction made. For a
//! receiver that has applied our earlier updates the two are equivalent — and
//! the transport guarantees exactly that, because pending edits are sequenced
//! per document (`local_sequence`) and compaction *merges* queued edits rather
//! than dropping any. Nothing relies on the re-send: full state is emitted
//! deliberately, via `force`/reseed paths, which still use `encode_diff_v1`
//! against an empty state vector.
//!
//! # Falling back
//!
//! [`UpdateCapture::record`] returns `None` rather than a wrong answer if it
//! cannot vouch for what it collected (today: a re-entrant capture, which no
//! caller does but which a future one might introduce silently). Callers then
//! take the `encode_diff_v1` path, so the failure mode is the old cost, never
//! an incomplete delta.

use std::sync::{Arc, Mutex};

use yrs::Doc;

/// Updates collected from a document's committed transactions.
#[derive(Default)]
struct CaptureSlot {
    /// Whether a [`UpdateCapture::record`] call is currently collecting. The
    /// observer fires on *every* commit — remote applies and re-seeds included —
    /// so anything committed outside a `record` must be ignored.
    armed: bool,
    updates: Vec<Vec<u8>>,
}

pub(crate) struct UpdateCapture {
    slot: Arc<Mutex<CaptureSlot>>,
}

impl UpdateCapture {
    /// Install the capturing observer on `doc`.
    ///
    /// Keyed, like the other observers on these documents, so it lives and dies
    /// with the document and needs no retained `Subscription`.
    pub(crate) fn install(doc: &Doc) -> Self {
        let slot: Arc<Mutex<CaptureSlot>> = Arc::default();
        let sink = Arc::clone(&slot);
        let _ = doc.observe_update_v1_with("knotq_update_capture", move |_txn, event| {
            let mut slot = sink.lock().unwrap_or_else(|err| err.into_inner());
            if slot.armed {
                slot.updates.push(event.update.clone());
            }
        });
        Self { slot }
    }

    /// Start collecting, for everything committed until the guard is finished
    /// or dropped.
    ///
    /// `None` means a capture is already running on this document and the
    /// caller must take the `encode_diff_v1` path instead — armed *before* the
    /// write, so the caller can still take the pre-write state vector the
    /// fallback needs.
    pub(crate) fn arm(&self) -> Option<CaptureGuard> {
        let mut slot = self.slot.lock().unwrap_or_else(|err| err.into_inner());
        if slot.armed {
            // Re-entrant: the outer capture owns the buffer, and taking it here
            // would silently truncate its delta.
            debug_assert!(false, "nested UpdateCapture::arm");
            return None;
        }
        slot.armed = true;
        slot.updates.clear();
        Some(CaptureGuard {
            slot: Arc::clone(&self.slot),
        })
    }

    /// Run `write`, returning its value alongside one update carrying exactly
    /// what it committed, or `None` if a capture was already running.
    #[cfg(test)]
    pub(crate) fn record<T>(
        &self,
        write: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<(T, Option<Vec<u8>>)> {
        let Some(guard) = self.arm() else {
            return write().map(|value| (value, None));
        };
        let value = write()?;
        Ok((value, Some(guard.finish()?)))
    }
}

/// How one reconcile will produce its outgoing delta, decided before the write
/// because the fallback needs the state vector as it was beforehand.
/// `Base` is however that caller spells a pre-write state vector — encoded
/// bytes for the scheme document, a `StateVector` for the workspace index.
pub(crate) enum Delta<Base> {
    /// Collecting the writes themselves.
    Captured(CaptureGuard),
    /// Falling back: diff against this pre-write state afterwards.
    Diff(Base),
}

/// An armed capture. Dropping it stops collecting, so a `?` between arming and
/// [`CaptureGuard::finish`] cannot leave the document collecting forever.
pub(crate) struct CaptureGuard {
    slot: Arc<Mutex<CaptureSlot>>,
}

impl CaptureGuard {
    /// One update equivalent to everything committed since arming. Empty when
    /// nothing was.
    pub(crate) fn finish(self) -> anyhow::Result<Vec<u8>> {
        let updates = {
            let mut slot = self.slot.lock().unwrap_or_else(|err| err.into_inner());
            slot.armed = false;
            std::mem::take(&mut slot.updates)
        };
        // Disarmed above; skip the Drop that would only disarm again.
        std::mem::forget(self);
        Ok(match updates.len() {
            0 => Vec::new(),
            1 => updates.into_iter().next().unwrap_or_default(),
            _ => yrs::merge_updates_v1(&updates)?,
        })
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        let mut slot = self.slot.lock().unwrap_or_else(|err| err.into_inner());
        slot.armed = false;
        slot.updates.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::updates::decoder::Decode;
    use yrs::{GetString, ReadTxn, StateVector, Text, Transact, Update};

    fn doc_with_text() -> (Doc, yrs::TextRef) {
        let doc = Doc::new();
        let text = doc.get_or_insert_text("body");
        (doc, text)
    }

    /// The captured delta must land a peer in the same place the diff it
    /// replaces would have — that equivalence is the whole claim.
    ///
    /// Both peers start from the document's pre-edit state, then one applies
    /// the captured delta and the other the `encode_diff_v1` it replaces.
    #[test]
    fn a_captured_delta_carries_the_same_change_as_a_diff() {
        let (doc, text) = doc_with_text();
        let capture = UpdateCapture::install(&doc);
        {
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, "hello");
        }
        // The shared base both peers start from, and the state the delta is
        // relative to.
        let base = doc.transact().encode_diff_v1(&StateVector::default());
        let before = doc.transact().state_vector();

        let (_, captured) = capture
            .record(|| {
                let mut txn = doc.transact_mut();
                text.insert(&mut txn, 5, " world");
                Ok(())
            })
            .unwrap();
        let captured = captured.expect("captured");
        let diffed = doc.transact().encode_diff_v1(&before);

        for delta in [&captured, &diffed] {
            let (peer, peer_text) = doc_with_text();
            let mut txn = peer.transact_mut();
            txn.apply_update(Update::decode_v1(&base).unwrap()).unwrap();
            txn.apply_update(Update::decode_v1(delta).unwrap()).unwrap();
            drop(txn);
            assert_eq!(peer_text.get_string(&peer.transact()), "hello world");
        }
    }

    /// A delete made inside a capture must reach a peer, even though the
    /// captured delta carries only that delete rather than the document's whole
    /// delete set the way `encode_diff_v1` did.
    #[test]
    fn a_delete_made_inside_a_capture_reaches_a_peer() {
        let (doc, text) = doc_with_text();
        let capture = UpdateCapture::install(&doc);
        {
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, "hello world");
        }
        let base = doc.transact().encode_diff_v1(&StateVector::default());

        let (_, captured) = capture
            .record(|| {
                let mut txn = doc.transact_mut();
                text.remove_range(&mut txn, 5, 6);
                Ok(())
            })
            .unwrap();

        let (peer, peer_text) = doc_with_text();
        let mut txn = peer.transact_mut();
        txn.apply_update(Update::decode_v1(&base).unwrap()).unwrap();
        txn.apply_update(Update::decode_v1(&captured.expect("captured")).unwrap())
            .unwrap();
        drop(txn);
        assert_eq!(peer_text.get_string(&peer.transact()), "hello");
    }

    /// Several transactions inside one `record` merge into a single delta.
    #[test]
    fn every_transaction_in_one_record_is_collected() {
        let (doc, text) = doc_with_text();
        let capture = UpdateCapture::install(&doc);

        let (_, captured) = capture
            .record(|| {
                for word in ["a", "b", "c"] {
                    let mut txn = doc.transact_mut();
                    let len = text.len(&txn);
                    text.insert(&mut txn, len, word);
                }
                Ok(())
            })
            .unwrap();

        let (peer, peer_text) = doc_with_text();
        peer.transact_mut()
            .apply_update(Update::decode_v1(&captured.expect("captured")).unwrap())
            .unwrap();
        assert_eq!(peer_text.get_string(&peer.transact()), "abc");
    }

    /// A commit outside `record` — a remote apply, a re-seed — must not end up
    /// in the next delta this device sends.
    #[test]
    fn commits_outside_a_record_are_not_captured() {
        let (doc, text) = doc_with_text();
        let capture = UpdateCapture::install(&doc);
        {
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, "remote");
        }

        let (_, captured) = capture
            .record(|| {
                let mut txn = doc.transact_mut();
                let len = text.len(&txn);
                text.insert(&mut txn, len, "!");
                Ok(())
            })
            .unwrap();

        // Applying to a peer that has none of it yields only the recorded edit;
        // the "remote" insert is missing, so the text cannot materialize.
        let update = Update::decode_v1(&captured.expect("captured")).unwrap();
        let (peer, peer_text) = doc_with_text();
        peer.transact_mut().apply_update(update).unwrap();
        assert_eq!(peer_text.get_string(&peer.transact()), "");
    }

    /// A write that changes nothing yields an empty delta, not a fallback.
    #[test]
    fn a_write_that_changes_nothing_captures_an_empty_delta() {
        let (doc, _text) = doc_with_text();
        let capture = UpdateCapture::install(&doc);

        let (_, captured) = capture.record(|| Ok(())).unwrap();

        assert_eq!(captured, Some(Vec::new()));
    }

    /// An error from the write propagates, and the next capture still works —
    /// the slot must not be left armed.
    #[test]
    fn a_failed_write_leaves_the_capture_usable() {
        let (doc, text) = doc_with_text();
        let capture = UpdateCapture::install(&doc);

        let failed: anyhow::Result<((), Option<Vec<u8>>)> =
            capture.record(|| Err(anyhow::anyhow!("boom")));
        assert!(failed.is_err());

        let (_, captured) = capture
            .record(|| {
                let mut txn = doc.transact_mut();
                text.insert(&mut txn, 0, "after");
                Ok(())
            })
            .unwrap();
        let (peer, peer_text) = doc_with_text();
        peer.transact_mut()
            .apply_update(Update::decode_v1(&captured.expect("captured")).unwrap())
            .unwrap();
        assert_eq!(peer_text.get_string(&peer.transact()), "after");
    }
}
