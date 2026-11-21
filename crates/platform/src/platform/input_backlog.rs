//! Fixed admission policy for retained per-window delivery.

use neocogi_platform::render::platform::MAX_TEXT_INPUT_BYTES;
use std::collections::VecDeque;

/// Maximum queued ordinary entries, excluding the reserved overflow and native close signals.
pub(super) const MAX_ENTRIES: usize = 4096;
/// Maximum retained text storage. Two maximum complete commits can be queued at once.
const MAX_TEXT_BYTES: usize = 2 * MAX_TEXT_INPUT_BYTES;
/// Maximum simultaneous fresh keys remembered for repeat recovery.
const MAX_TRACKED_KEYS: usize = 256;

/// A detached ordinary transition or the cancellation reserved after an admitted prefix.
pub(super) enum Delivery<T> {
    /// One accepted input transition, detached from its queue budget.
    Entry(T),
    /// Reserved reset boundary after the accepted FIFO prefix.
    Overflow,
}

/// FIFO bound by both entry count and payload allocation, with one reserved overflow marker.
pub(super) struct Backlog<T> {
    /// Each transition keeps its payload charge until detachment.
    entries: VecDeque<(T, usize)>,
    /// Sum of payload allocations currently retained in the FIFO.
    text_bytes: usize,
    /// A pending overflow marker blocks admission without allocating a slot.
    saturated: bool,
}

impl<T> Default for Backlog<T> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            text_bytes: 0,
            saturated: false,
        }
    }
}

impl<T> Backlog<T> {
    /// Native close uses the current prefix length to preserve its insertion position.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Overflow stops admission until the marker is detached for delivery.
    pub(super) fn saturated(&self) -> bool {
        self.saturated
    }

    /// An adjacent zero-payload motion can be replaced without charging another entry.
    pub(super) fn tail_mut(&mut self) -> Option<&mut T> {
        if self.saturated {
            return None;
        }
        self.entries.back_mut().and_then(|(event, bytes)| (*bytes == 0).then_some(event))
    }

    /// Returns whether the event was retained. On first refusal, reserve a marker without allocation.
    pub(super) fn push(&mut self, event: T, bytes: usize) -> bool {
        if self.saturated || self.entries.len() == MAX_ENTRIES || bytes > MAX_TEXT_BYTES - self.text_bytes {
            self.saturated = true;
            return false;
        }
        self.entries.push_back((event, bytes));
        self.text_bytes += bytes;
        true
    }

    /// Releases the detached transition's charge before the calling host can enter foreign code.
    pub(super) fn pop(&mut self) -> Option<Delivery<T>> {
        if let Some((event, bytes)) = self.entries.pop_front() {
            self.text_bytes -= bytes;
            return Some(Delivery::Entry(event));
        }
        std::mem::take(&mut self.saturated).then_some(Delivery::Overflow)
    }
}

/// Once history was lost, only repeats paired with a new admitted press reach the input sink.
pub(super) struct KeyRecovery<K> {
    /// Absent on the ordinary path. The fixed limit bounds distinct physical identities.
    fresh: Option<Vec<K>>,
}

impl<K> Default for KeyRecovery<K> {
    fn default() -> Self {
        Self { fresh: None }
    }
}

impl<K: Eq> KeyRecovery<K> {
    /// Forget every press from the discarded input history.
    pub(super) fn reset(&mut self) {
        self.fresh.get_or_insert_default().clear();
    }

    /// Whether the adapter needs to acquire a physical identity for the next key event.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn recovering(&self) -> bool {
        self.fresh.is_some()
    }

    /// Only an admitted fresh press can authorize a repeat after reset.
    pub(super) fn allows_repeat(&self, key: Option<&K>) -> bool {
        self.fresh.as_ref().is_none_or(|fresh| key.is_some_and(|key| fresh.contains(key)))
    }

    /// Records a known physical down after it is admitted to the logical delivery path.
    pub(super) fn press(&mut self, key: Option<K>) {
        if let (Some(fresh), Some(key)) = (&mut self.fresh, key)
            && fresh.len() < MAX_TRACKED_KEYS
            && !fresh.contains(&key)
        {
            fresh.push(key);
        }
    }

    /// Forgets the physical key even when the adapter cannot produce a logical release.
    pub(super) fn release(&mut self, key: Option<&K>) {
        if let (Some(fresh), Some(key)) = (&mut self.fresh, key)
            && let Some(index) = fresh.iter().position(|candidate| candidate == key)
        {
            fresh.swap_remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Overflow recovery stays bounded and permits repeats only while a tracked key is held.
    #[test]
    fn physical_repeat_recovery_stays_bounded() {
        let mut keys = KeyRecovery::default();
        assert!(keys.allows_repeat(None));
        keys.reset();
        keys.press(None);
        assert!(!keys.allows_repeat(None));
        for key in 0..MAX_TRACKED_KEYS {
            keys.press(Some(key));
            assert!(keys.allows_repeat(Some(&key)));
        }
        keys.press(Some(MAX_TRACKED_KEYS));
        assert!(!keys.allows_repeat(Some(&MAX_TRACKED_KEYS)));
        keys.release(Some(&1));
        assert!(!keys.allows_repeat(Some(&1)));
        keys.press(Some(MAX_TRACKED_KEYS));
        assert!(keys.allows_repeat(Some(&MAX_TRACKED_KEYS)));
        keys.reset();
        assert!(!keys.allows_repeat(Some(&MAX_TRACKED_KEYS)));
    }

    /// Each budget preserves the admitted prefix and one marker, then accepts new work.
    #[test]
    fn independent_count_and_byte_refusals_have_one_ordered_marker() {
        let mut entries = Backlog::default();
        for number in 0..MAX_ENTRIES {
            assert!(entries.push(number, 0));
        }
        assert!(!entries.push(MAX_ENTRIES, 0));
        assert!(entries.tail_mut().is_none());
        assert!(matches!(entries.pop(), Some(Delivery::Entry(0))));
        assert!(!entries.push(MAX_ENTRIES + 1, 0));
        for number in 1..MAX_ENTRIES {
            assert!(matches!(entries.pop(), Some(Delivery::Entry(value)) if value == number));
        }
        assert!(matches!(entries.pop(), Some(Delivery::Overflow)));
        assert!(entries.pop().is_none());

        assert!(entries.push(1, MAX_TEXT_INPUT_BYTES));
        assert!(entries.push(2, MAX_TEXT_INPUT_BYTES));
        assert!(!entries.push(3, 1));
        assert!(matches!(entries.pop(), Some(Delivery::Entry(1))));
        assert!(!entries.push(4, 1));
        assert!(matches!(entries.pop(), Some(Delivery::Entry(2))));
        assert!(matches!(entries.pop(), Some(Delivery::Overflow)));
        assert!(entries.push(5, MAX_TEXT_INPUT_BYTES));
        assert!(matches!(entries.pop(), Some(Delivery::Entry(5))));
        assert!(entries.pop().is_none());
    }
}
