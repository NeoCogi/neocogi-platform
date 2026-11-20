//! Fixed admission policy for retained per-window delivery.

use neocogi_platform::render::platform::{KeyAction, KeyCode, KeyStroke, MAX_TEXT_INPUT_BYTES};
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
        Self { entries: VecDeque::new(), text_bytes: 0, saturated: false }
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
#[derive(Default)]
pub(super) struct KeyRecovery {
    /// Absent on the ordinary path. The fixed limit also bounds synthetic distinct logical keys.
    fresh: Option<Vec<KeyCode>>,
}

impl KeyRecovery {
    /// Forget every press from the discarded input history.
    pub(super) fn reset(&mut self) {
        self.fresh.get_or_insert_default().clear();
    }

    /// Admit a key or suppress a repeat belonging to history before the last reset.
    pub(super) fn accepts(&mut self, stroke: KeyStroke) -> bool {
        let Some(fresh) = &mut self.fresh else { return true };
        let index = fresh.iter().position(|key| *key == stroke.key);
        match stroke.action {
            KeyAction::Pressed { repeat: true } => index.is_some(),
            KeyAction::Pressed { repeat: false } => {
                if index.is_none() && fresh.len() < MAX_TRACKED_KEYS {
                    fresh.push(stroke.key);
                }
                true
            }
            KeyAction::Released => {
                if let Some(index) = index {
                    fresh.swap_remove(index);
                }
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
