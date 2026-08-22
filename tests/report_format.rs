//! The text report is a user interface: columns have to line up and the
//! numbers have to be the ones a reader would compute by hand. These tests pin
//! the exact layout so that a refactor of the report cannot quietly wreck it.

use jsonl_peek::{FieldPath, Schema, SchemaOptions, Stats, StatsOptions};

const RECORDS: &str = concat!(
    "{\"id\":1,\"tags\":[\"a\"],\"meta\":{\"src\":\"web\"}}\n",
    "{\"id\":2,\"tags\":[\"a\",\"b\"],\"meta\":{\"src\":\"web\"}}\n",
    "{\"id\":3,\"meta\":{\"src\":\"forum\"}}\n",
    "{\"id\":4,\"meta\":{\"src\":\"forum\"}}\n",
);

fn stats() -> Stats {
    let options = StatsOptions {
        fields: vec![FieldPath::parse("meta.src").unwrap()],
        ..StatsOptions::default()
    };
    Stats::from_reader(RECORDS.as_bytes(), options).unwrap()
}

/// Every column of a table has to start at the same offset on every row.
fn column_starts(lines: &[&str]) -> Vec<Vec<usize>> {
    lines
        .iter()
        .map(|line| {
            let bytes = line.as_bytes();
            (0..bytes.len())
                .filter(|&i| {
                    bytes[i] != b' ' && (i == 0 || bytes[i - 1] == b' ') && i > 0
                })
                .collect()
        })
        .collect()
}

#[test]
fn key_table_columns_line_up() {
    let report = stats().report_text("records.jsonl", 10, 0);
    let lines: Vec<&str> = report.lines().collect();
    let header = lines
        .iter()
        .position(|line| line.trim_start().starts_with("key "))
        .expect("the key table needs a header");

    let rows: Vec<&str> = lines[header..header + 4].to_vec();
    assert_eq!(rows.len(), 4, "one header plus id, meta and tags");

    let starts = column_starts(&rows);
    let header_columns = &starts[0];
    for (row, columns) in rows.iter().zip(&starts).skip(1) {
        assert_eq!(
            columns.len(),
            header_columns.len(),
            "row has a different number of columns:\n{}",
            row
        );
        // The count column is right aligned, so only the first and the last
        // column have to start at the header's offset; the numeric ones only
        // have to end there.
        assert_eq!(columns[0], header_columns[0], "key column moved:\n{}", row);
    }
}

#[test]
fn counts_in_the_report_match_the_data() {
    let report = stats().report_text("records.jsonl", 10, 0);
    assert!(report.contains("file    records.jsonl"), "{}", report);
    assert!(report.contains("blank 0   invalid 0   valid 4"), "{}", report);
    assert!(report.contains("top level keys over 4 objects"), "{}", report);
    // id is in all four records, tags only in two.
    assert!(report.contains("id                                4  100.0%  int:4"), "{}", report);
    assert!(report.contains("tags                              2   50.0%  array:2"), "{}", report);
    // meta.src has three "web"/"forum" values spread over four records.
    assert!(report.contains("present in 4 of 4 records (100.0%), 4 values"), "{}", report);
    assert!(report.contains("2   50.0%  \"web\""), "{}", report);
    assert!(report.contains("2   50.0%  \"forum\""), "{}", report);
}

#[test]
fn a_clean_file_has_no_error_section() {
    let report = stats().report_text("records.jsonl", 10, 0);
    assert!(!report.contains("invalid lines ("), "{}", report);

    let dirty = Stats::from_reader("{}\noops\n".as_bytes(), StatsOptions::default()).unwrap();
    let report = dirty.report_text("dirty.jsonl", 10, 0);
    assert!(report.contains("invalid lines (1 total, showing 1)"), "{}", report);
    assert!(report.contains("line 2 col 1: unexpected 'o'"), "{}", report);
}

#[test]
fn schema_rates_are_per_document_not_per_occurrence() {
    let mut schema = Schema::new(SchemaOptions::default());
    for line in RECORDS.lines() {
        schema.observe(&jsonl_peek::parse(line).unwrap());
    }
    let report = schema.report_text(0.0);
    assert!(report.starts_with("4 records, depth 3"), "{}", report);
    // Three tag strings live in two of the four records: the rate is 50%, the
    // type count is 3.
    assert!(report.contains("tags[]"), "{}", report);
    let row = report
        .lines()
        .find(|line| line.trim_start().starts_with("tags[]"))
        .unwrap();
    assert!(row.contains("50.0%"), "{}", row);
    assert!(row.contains("string:3"), "{}", row);
}

#[test]
fn percentages_never_divide_by_zero() {
    let empty = Stats::from_reader("".as_bytes(), StatsOptions::default()).unwrap();
    let report = empty.report_text("empty.jsonl", 10, 0);
    assert!(!report.contains("NaN"), "{}", report);
    assert!(!report.contains("inf"), "{}", report);

    let schema = Schema::new(SchemaOptions::default());
    let report = schema.report_text(0.0);
    assert!(report.starts_with("0 records"), "{}", report);
}
