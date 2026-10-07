//! The original-rules ledger is the binding record of what is evidence,
//! policy and unknown. Other docs cite rows by ID, so an ID that appears on
//! two rows silently makes every citation ambiguous.

use std::collections::BTreeMap;
use std::path::Path;

/// The ID in the first cell of a ledger table row (`| DSN-88 | …`).
fn row_id(line: &str) -> Option<&str> {
    let cell = line.strip_prefix('|')?.split('|').next()?.trim();
    let (prefix, number) = cell.rsplit_once('-')?;
    let tagged = !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_uppercase());
    (tagged && !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then_some(cell)
}

fn duplicates(ledger: &str) -> Vec<(String, Vec<usize>)> {
    let mut seen: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, line) in ledger.lines().enumerate() {
        if let Some(id) = row_id(line) {
            seen.entry(id).or_default().push(i + 1);
        }
    }
    seen.into_iter()
        .filter(|(_, lines)| lines.len() > 1)
        .map(|(id, lines)| (id.to_string(), lines))
        .collect()
}

#[test]
fn every_ledger_id_labels_exactly_one_row() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/original-rules.md");
    let ledger = std::fs::read_to_string(&path).expect("read docs/original-rules.md");
    let dupes = duplicates(&ledger);
    assert!(
        dupes.is_empty(),
        "ledger IDs used on more than one row (id, lines): {dupes:?}"
    );
}

#[test]
fn the_check_finds_a_repeated_id_and_ignores_prose() {
    let doc = "| DSN-1 | a |\n| DSN-2 | b |\n| DSN-1 | c |\n| not-an-id | d |\n| not-an-id | e |\n|---|---|\ntext | DSN-2 |\n";
    assert_eq!(duplicates(doc), vec![("DSN-1".to_string(), vec![1, 3])]);
}
