//! Shared UTF-8 ownership policy for independently retained audio/render diagnostics.

/// Bounds both visible bytes and retained String capacity, preserving complete UTF-8 scalars.
/// Already bounded allocations move unchanged. Oversized storage, including spare capacity
/// behind a short/empty string, is released after copying only the admitted prefix.
/// The caller's conversion into String precedes this retained-storage guarantee.
pub(super) fn bounded_message(message: String, maximum: usize) -> String {
    if message.capacity() <= maximum {
        return message;
    }
    let mut end = message.len().min(maximum);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every cut around one-, two- and four-byte scalars has an independent expected prefix.
    /// Excess capacity must disappear even when the whole visible string fits the limit.
    #[test]
    fn utf8_prefix_and_owned_capacity_share_the_same_bound() {
        for (maximum, expected) in [
            (0, ""),
            (1, "a"),
            (2, "a"),
            (3, "aé"),
            (4, "aé"),
            (5, "aé"),
            (6, "aé"),
            (7, "aé🦀"),
            (8, "aé🦀z"),
            (9, "aé🦀z"),
        ] {
            for capacity in [8, 1024] {
                let mut input = String::with_capacity(capacity);
                input.push_str("aé🦀z");
                let output = bounded_message(input, maximum);
                assert_eq!(output, expected);
                assert!(output.capacity() <= maximum);
            }
        }
        let empty = bounded_message(String::with_capacity(1024), 0);
        assert!(empty.is_empty());
        assert_eq!(empty.capacity(), 0);
    }
}
