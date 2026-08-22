//! End to end tests that run the real binary against a real file on disk.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use jsonl_peek::json::{parse, Value};

/// Path to the binary cargo just built for us.
const BIN: &str = env!("CARGO_BIN_EXE_jsonl-peek");

/// Six good records, one blank line and one syntactically broken line.
const FIXTURE: &str = concat!(
    "{\"id\":1,\"role\":\"user\",\"text\":\"hello\",\"meta\":{\"src\":\"web\"}}\n",
    "{\"id\":2,\"role\":\"assistant\",\"text\":\"hi there\",\"meta\":{\"src\":\"web\"}}\n",
    "{\"id\":3,\"role\":\"user\",\"text\":\"how are you\",\"meta\":{\"src\":\"forum\"}}\n",
    "\n",
    "{\"id\":4,\"role\":\"assistant\",\"text\":\"fine\",\"meta\":{\"src\":\"forum\"}}\n",
    "{\"id\":5,\"role\":\"user\",\"text\":\"bye\"}\n",
    "{\"id\":6,\"role\":\"assistant\" \"text\":\"later\"}\n",
    "{\"id\":7,\"role\":\"user\",\"text\":\"again\",\"meta\":{\"src\":\"web\"}}\n",
);

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(name: &str, contents: &str) -> Fixture {
        let mut path = std::env::temp_dir();
        path.push(format!("jsonl-peek-{}-{}.jsonl", std::process::id(), name));
        std::fs::write(&path, contents).expect("write fixture");
        Fixture { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().expect("run binary")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

#[test]
fn stats_summarises_the_file() {
    let fixture = Fixture::new("stats", FIXTURE);
    let out = run(&["stats", fixture.path().to_str().unwrap()]);
    assert!(out.status.success(), "stats should succeed");
    let text = stdout_of(&out);

    assert!(text.contains("lines"), "report:\n{}", text);
    assert!(text.contains("blank 1"), "report:\n{}", text);
    assert!(text.contains("invalid 1"), "report:\n{}", text);
    assert!(text.contains("valid 6"), "report:\n{}", text);
    assert!(text.contains("top level keys over 6 objects"), "report:\n{}", text);
    // The broken record is line 7 and the quote after "assistant" is column 28.
    assert!(text.contains("line 7 col 28"), "report:\n{}", text);
}

#[test]
fn stats_json_is_machine_readable() {
    let fixture = Fixture::new("statsjson", FIXTURE);
    let out = run(&[
        "stats",
        "--json",
        "--field",
        "meta.src",
        fixture.path().to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let report = parse(&stdout_of(&out)).expect("--json output must be valid JSON");

    assert_eq!(report.get("lines").and_then(Value::as_i64), Some(8));
    assert_eq!(report.get("valid").and_then(Value::as_i64), Some(6));
    assert_eq!(report.get("invalid").and_then(Value::as_i64), Some(1));
    assert_eq!(report.get("blank").and_then(Value::as_i64), Some(1));

    let role = report
        .get("keys")
        .and_then(|keys| keys.get("role"))
        .and_then(|role| role.get("count"))
        .and_then(Value::as_i64);
    assert_eq!(role, Some(6));

    let field = &report.get("fields").and_then(Value::as_array).unwrap()[0];
    assert_eq!(field.get("path").and_then(Value::as_str), Some("meta.src"));
    assert_eq!(field.get("values").and_then(Value::as_i64), Some(5));
    let top = field.get("top").and_then(Value::as_array).unwrap();
    assert_eq!(top[0].get("value").and_then(Value::as_str), Some("\"web\""));
    assert_eq!(top[0].get("count").and_then(Value::as_i64), Some(3));
}

#[test]
fn stats_min_count_hides_rare_field_values() {
    let fixture = Fixture::new("mincount", FIXTURE);
    let path = fixture.path().to_str().unwrap();
    // meta.src is "web" 3 times and "forum" twice across the 6 valid records.
    let all = stdout_of(&run(&["stats", "--field", "meta.src", path]));
    assert!(all.contains("\"web\""));
    assert!(all.contains("\"forum\""));

    let common = stdout_of(&run(&[
        "stats", "--field", "meta.src", "--min-count", "3", path,
    ]));
    assert!(common.contains("\"web\""), "report:\n{}", common);
    assert!(!common.contains("\"forum\""), "report:\n{}", common);
}

#[test]
fn head_prints_the_first_lines() {
    let fixture = Fixture::new("head", FIXTURE);
    let out = run(&["head", "-n", "2", fixture.path().to_str().unwrap()]);
    assert!(out.status.success());
    let text = stdout_of(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("\"id\":1"));
    assert!(lines[1].contains("\"id\":2"));
}

#[test]
fn sample_is_bounded_and_reproducible() {
    let fixture = Fixture::new("sample", FIXTURE);
    let path = fixture.path().to_str().unwrap();
    let first = stdout_of(&run(&["sample", "-n", "3", "--seed", "42", path]));
    let again = stdout_of(&run(&["sample", "-n", "3", "--seed", "42", path]));
    assert_eq!(first, again, "the same seed must give the same sample");

    let picked: Vec<&str> = first.lines().collect();
    assert_eq!(picked.len(), 3);
    let source: Vec<&str> = FIXTURE.lines().collect();
    for line in &picked {
        assert!(source.contains(line), "sampled line is not from the file: {}", line);
        assert!(!line.is_empty(), "blank lines must never be sampled");
    }
    // File order is preserved in the output.
    let mut positions: Vec<usize> = picked
        .iter()
        .map(|line| source.iter().position(|s| s == line).unwrap())
        .collect();
    let sorted = {
        let mut copy = positions.clone();
        copy.sort_unstable();
        copy
    };
    positions.dedup();
    assert_eq!(positions.len(), 3, "no line may be sampled twice");
    assert_eq!(positions, sorted);
}

#[test]
fn sample_of_a_short_file_returns_everything() {
    let fixture = Fixture::new("short", "{\"a\":1}\n{\"a\":2}\n");
    let out = run(&["sample", "-n", "50", fixture.path().to_str().unwrap()]);
    assert_eq!(stdout_of(&out), "{\"a\":1}\n{\"a\":2}\n");
}

#[test]
fn schema_infers_nested_paths() {
    let fixture = Fixture::new("schema", FIXTURE);
    let out = run(&["schema", "--depth", "2", fixture.path().to_str().unwrap()]);
    assert!(out.status.success());
    let text = stdout_of(&out);
    assert!(text.starts_with("6 records, depth 2"), "report:\n{}", text);
    assert!(text.contains("meta.src"), "report:\n{}", text);
    assert!(text.contains("1 unparseable lines skipped"), "report:\n{}", text);
}

#[test]
fn schema_json_reports_skipped_lines() {
    let fixture = Fixture::new("schemajson", FIXTURE);
    let out = run(&["schema", "--json", fixture.path().to_str().unwrap()]);
    assert!(out.status.success());
    let report = parse(&stdout_of(&out)).expect("--json output must be valid JSON");

    assert_eq!(report.get("records").and_then(Value::as_i64), Some(6));
    assert_eq!(report.get("skipped").and_then(Value::as_i64), Some(1));
}

#[test]
fn schema_min_rate_hides_rare_paths() {
    let fixture = Fixture::new("minrate", FIXTURE);
    let path = fixture.path().to_str().unwrap();
    let all = stdout_of(&run(&["schema", path]));
    let common = stdout_of(&run(&["schema", "--min-rate", "0.9", path]));
    assert!(all.contains("meta.src"));
    // meta.src is present in 5 of 6 records, so 0.9 filters it out.
    assert!(!common.contains("meta.src"), "report:\n{}", common);
    assert!(common.contains("role"));
}

#[test]
fn reads_standard_input_when_no_file_is_given() {
    let mut child = Command::new(BIN)
        .args(["stats", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(FIXTURE.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    let report = parse(&stdout_of(&out)).unwrap();
    assert_eq!(report.get("file").and_then(Value::as_str), Some("-"));
    assert_eq!(report.get("valid").and_then(Value::as_i64), Some(6));
}

#[test]
fn usage_errors_exit_with_two() {
    let out = run(&["frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown command"));

    let out = run(&["stats", "--field", "a..b"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("bad field path"));

    let out = run(&["head", "-n"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn missing_files_exit_with_one() {
    let out = run(&["stats", "/definitely/not/here.jsonl"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot open"));
}

#[test]
fn help_and_version_work() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    let text = stdout_of(&help);
    assert!(text.contains("usage:"));
    assert!(!text.contains("{prog}"), "the program name was not substituted");

    let version = run(&["--version"]);
    assert!(version.status.success());
    assert!(stdout_of(&version).trim().ends_with(env!("CARGO_PKG_VERSION")));
}
