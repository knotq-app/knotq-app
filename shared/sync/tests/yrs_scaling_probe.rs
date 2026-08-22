//! Is the cost of building a big scheme document in yrs itself, or in our code
//! around it? Times raw yrs operations at increasing sizes so the two can be
//! told apart.
//!
//! Run: cargo test -p knotq-sync --test yrs_scaling_probe --release -- --ignored --nocapture

use std::time::Instant;

use yrs::types::text::TextPrelim;
use yrs::types::map::MapPrelim;
use yrs::{Doc, Map, Transact};

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn probe_yrs_map_insert_scaling() {
    println!();
    println!("{:>7} | {:>14} {:>18} {:>16}", "items", "flat inserts", "nested map+text", "per item (us)");
    for &n in &[500usize, 1_000, 2_000, 4_000, 8_000] {
        // A: n flat string entries in one map.
        let doc = Doc::new();
        let map = doc.get_or_insert_map("flat");
        let start = Instant::now();
        {
            let mut txn = doc.transact_mut();
            for i in 0..n {
                map.insert(&mut txn, i.to_string(), "value");
            }
        }
        let flat = ms(start);

        // B: n nested maps, each holding a Text — the shape a scheme document
        // actually uses (one sub-map per item, with the body as a Y.Text).
        let doc = Doc::new();
        let map = doc.get_or_insert_map("items_by_id");
        let start = Instant::now();
        {
            let mut txn = doc.transact_mut();
            for i in 0..n {
                let entry = map.insert(&mut txn, i.to_string(), MapPrelim::default());
                entry.insert(&mut txn, "schema", "knotq.item.v1");
                entry.insert(&mut txn, "position", "a0");
                entry.insert(&mut txn, "text", TextPrelim::new("lorem ipsum dolor sit amet"));
            }
        }
        let nested = ms(start);

        println!(
            "{n:>7} | {flat:>12.1}ms {nested:>16.1}ms {:>14.1}",
            nested * 1000.0 / n as f64
        );
    }
}
