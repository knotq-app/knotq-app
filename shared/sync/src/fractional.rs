//! Fractional indexing: string keys that sort lexicographically and allow a new
//! key to be generated strictly between any two existing keys.
//!
//! Ordering CRDT items/nodes with a per-node key (instead of a shared array)
//! means concurrent inserts never collide destructively: two replicas that
//! independently generate a key in the same gap may produce the *same* string,
//! which is harmless — the call site breaks ties with the node's globally unique
//! id, so the total order stays deterministic and convergent.
//!
//! The algorithm is the well-known `generateKeyBetween` midpoint scheme. Keys
//! are strings over a base-62 digit alphabet whose byte order matches its index
//! order, so lexicographic string comparison equals digit comparison.

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn digit_index(byte: u8) -> Option<usize> {
    DIGITS.iter().position(|&d| d == byte)
}

/// Returns a key `k` with `a < k < b` lexicographically, where `a` defaults to
/// the smallest key (`""`) and `b` to the largest (`None`).
///
/// Callers must pass `a < b` when both are `Some`; equal/!inverted neighbors are
/// a call-site concern (handled there by re-spreading). The result never ends in
/// the lowest digit, so a key can always be generated on either side of it.
pub fn between(a: Option<&str>, b: Option<&str>) -> String {
    midpoint(a.unwrap_or(""), b)
}

fn midpoint(a: &str, b: Option<&str>) -> String {
    if let Some(b) = b {
        // Strip the longest common prefix, padding `a` with the zero digit once
        // it runs out, then recurse on the remainder.
        let a_bytes = a.as_bytes();
        let b_bytes = b.as_bytes();
        let mut n = 0;
        while n < b_bytes.len() {
            let ad = if n < a_bytes.len() {
                a_bytes[n]
            } else {
                DIGITS[0]
            };
            if ad == b_bytes[n] {
                n += 1;
            } else {
                break;
            }
        }
        if n > 0 {
            let a_rest = if n < a.len() { &a[n..] } else { "" };
            return format!("{}{}", &b[..n], midpoint(a_rest, Some(&b[n..])));
        }
    }

    let digit_a = a.bytes().next().and_then(digit_index).unwrap_or(0);
    let digit_b = match b {
        Some(b) if !b.is_empty() => digit_index(b.as_bytes()[0]).unwrap_or(DIGITS.len()),
        Some(_) => 0,
        None => DIGITS.len(),
    };

    if digit_b - digit_a > 1 {
        // Room for a digit strictly between the two; pick the midpoint.
        let mid = (digit_a + digit_b).div_ceil(2);
        (DIGITS[mid] as char).to_string()
    } else if let Some(b) = b.filter(|b| b.len() > 1) {
        // Consecutive leading digits but `b` has more: its first digit alone is
        // greater than `a` and less than `b`.
        b[..1].to_string()
    } else {
        // `b` is unbounded (or a single digit one above `a`): keep `a`'s leading
        // digit and find a key after its remainder.
        let a_rest = if a.len() > 1 { &a[1..] } else { "" };
        format!("{}{}", DIGITS[digit_a] as char, midpoint(a_rest, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn between_is_strictly_ordered_for_bounds() {
        let mid = between(None, None);
        assert!(!mid.is_empty());

        let before = between(None, Some(&mid));
        assert!(before < mid, "{before} < {mid}");

        let after = between(Some(&mid), None);
        assert!(mid < after, "{mid} < {after}");

        let inner = between(Some(&before), Some(&mid));
        assert!(before < inner && inner < mid, "{before} < {inner} < {mid}");
    }

    #[test]
    fn between_never_ends_in_zero_digit() {
        // Repeatedly insert at the front and back; no generated key should end in
        // the lowest digit (which would leave no room to insert below it).
        let mut low = between(None, None);
        let mut high = low.clone();
        for _ in 0..200 {
            low = between(None, Some(&low));
            high = between(Some(&high), None);
            assert!(!low.ends_with('0'));
            assert!(!high.ends_with('0'));
        }
    }

    #[test]
    fn random_inserts_stay_strictly_sorted_and_distinct() {
        // Deterministic LCG so the test is reproducible without a dependency.
        let mut seed: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };

        let mut keys: Vec<String> = Vec::new();
        for _ in 0..1000 {
            let gap = if keys.is_empty() {
                0
            } else {
                next() % (keys.len() + 1)
            };
            let lo = if gap == 0 {
                None
            } else {
                Some(keys[gap - 1].as_str())
            };
            let hi = if gap == keys.len() {
                None
            } else {
                Some(keys[gap].as_str())
            };
            let key = between(lo, hi);
            keys.insert(gap, key);
        }

        for pair in keys.windows(2) {
            assert!(
                pair[0] < pair[1],
                "not strictly sorted: {} !< {}",
                pair[0],
                pair[1]
            );
        }
    }
}
