//! Command line front end.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use jsonl_peek::{
    FieldPath, LineReader, Reservoir, Schema, SchemaOptions, Stats, StatsOptions, VERSION,
};

/// Program name as installed. Replaced when the template is instantiated.
const PROG: &str = "jsonl-peek";

/// Read buffer size. Large enough that reading a multi-gigabyte file is
/// dominated by the parser rather than by syscalls.
const READ_BUFFER: usize = 256 * 1024;

/// Lines between `--progress` updates. `stats` and `schema` already track
/// `lines_read`/`bytes_read` per line, so reporting on them costs nothing
/// beyond the flag check itself and does not touch the single-pass,
/// bounded-memory accumulators.
const PROGRESS_INTERVAL: u64 = 10_000;

/// Emits a periodic status line to stderr on a long run, so `stats` or
/// `schema` on a multi-gigabyte file is not silent for minutes at a time.
struct Progress {
    every: u64,
    next: u64,
}

impl Progress {
    fn new(every: u64) -> Self {
        Progress { every, next: every }
    }

    /// Reports once `lines` has crossed the next threshold. Threshold is
    /// advanced past `lines` rather than by a fixed step, so a burst of many
    /// lines between calls still reports just once.
    fn tick(&mut self, lines: u64, bytes: u64) {
        if lines < self.next {
            return;
        }
        eprintln!("... {} lines, {} bytes read", lines, bytes);
        while self.next <= lines {
            self.next += self.every;
        }
    }
}

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(Fail::Usage(message)) => {
            eprintln!("{}: {}", PROG, message);
            eprintln!("try '{} --help'", PROG);
            2
        }
        Err(Fail::Message(message)) => {
            eprintln!("{}: {}", PROG, message);
            1
        }
        Err(Fail::Io(err)) if err.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(Fail::Io(err)) => {
            eprintln!("{}: {}", PROG, err);
            1
        }
    };
    std::process::exit(code);
}

enum Fail {
    Usage(String),
    Message(String),
    Io(io::Error),
}

impl From<io::Error> for Fail {
    fn from(err: io::Error) -> Self {
        Fail::Io(err)
    }
}

fn run() -> Result<(), Fail> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        print_usage();
        return Err(Fail::Usage("missing command".to_string()));
    };
    match command.as_str() {
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        "-V" | "--version" => {
            println!("{} {}", PROG, VERSION);
            Ok(())
        }
        "head" => cmd_head(&args[1..]),
        "sample" => cmd_sample(&args[1..]),
        "stats" => cmd_stats(&args[1..]),
        "schema" => cmd_schema(&args[1..]),
        other => Err(Fail::Usage(format!("unknown command '{}'", other))),
    }
}

fn print_usage() {
    let text = "\
{prog} - quick health checks for JSONL datasets

usage:
  {prog} head   [-n N] [FILE]
  {prog} sample [-n N] [--seed S] [FILE]
  {prog} stats  [--field PATH]... [--top N] [--min-count N] [--max-errors N] [--json] [--progress] [--fail-on-invalid] [FILE]
  {prog} schema [--depth N] [--min-rate R] [--json] [--progress] [--fail-on-invalid] [FILE]

FILE defaults to '-', meaning standard input. Every command reads the input
exactly once and keeps a bounded amount of state, so it is safe to point at a
file that is far larger than memory.

commands:
  head     print the first N lines (default 10)
  sample   uniform random sample of N lines via reservoir sampling
  stats    line counts, byte counts, length percentiles, top level key and
           type frequencies, the position of every unparseable line, and
           optionally the value distribution of one or more fields
  schema   infer which dotted paths exist and which types they hold

options:
  -n N            number of lines (head, sample)
  --seed S        seed the sampler for a reproducible sample
  --field PATH    profile a field, e.g. 'meta.source' or 'messages[].role'
                  (repeatable)
  --top N         how many distinct values to list per field (default 10)
  --min-count N   hide field values that occur fewer than N times (default 0)
  --max-errors N  how many broken lines to show (default 10)
  --depth N       how deep to infer the schema (default 3)
  --min-rate R    hide schema paths present in fewer than R of the records,
                  R between 0 and 1 (default 0)
  --json          machine readable output (stats, schema)
  --progress      print a line count to stderr every {progress_interval}
                  lines (stats, schema)
  --fail-on-invalid  exit with status 1 if any line failed to parse
                  (stats, schema), after printing the report
  -h, --help      this text
  -V, --version   version

exit status: 0 success, 1 runtime error, 2 usage error
";
    print!(
        "{}",
        text.replace("{prog}", PROG)
            .replace("{progress_interval}", &PROGRESS_INTERVAL.to_string())
    );
}

fn open(path: &str) -> Result<Box<dyn BufRead>, Fail> {
    if path == "-" {
        return Ok(Box::new(BufReader::with_capacity(
            READ_BUFFER,
            io::stdin(),
        )));
    }
    let file = File::open(path)
        .map_err(|err| Fail::Message(format!("cannot open '{}': {}", path, err)))?;
    Ok(Box::new(BufReader::with_capacity(READ_BUFFER, file)))
}

fn emit(text: &str) -> Result<(), Fail> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(text.as_bytes())?;
    handle.flush()?;
    Ok(())
}

fn need_value<'a>(next: Option<&'a String>, flag: &str) -> Result<&'a String, Fail> {
    next.ok_or_else(|| Fail::Usage(format!("{} needs a value", flag)))
}

fn parse_number<T: std::str::FromStr>(text: &str, flag: &str) -> Result<T, Fail> {
    text.parse::<T>()
        .map_err(|_| Fail::Usage(format!("invalid value for {}: '{}'", flag, text)))
}

/// Splits the trailing positional argument off a flag list.
fn take_positional(current: &mut Option<String>, value: &str) -> Result<(), Fail> {
    if current.is_some() {
        return Err(Fail::Usage(format!("unexpected extra argument '{}'", value)));
    }
    *current = Some(value.to_string());
    Ok(())
}

fn unknown_flag(flag: &str) -> Fail {
    Fail::Usage(format!("unknown option '{}'", flag))
}

fn is_flag(arg: &str) -> bool {
    arg.starts_with('-') && arg != "-"
}

fn cmd_head(args: &[String]) -> Result<(), Fail> {
    let mut count = 10usize;
    let mut file: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-n" | "--lines" => count = parse_number(need_value(iter.next(), "-n")?, "-n")?,
            other if is_flag(other) => return Err(unknown_flag(other)),
            other => take_positional(&mut file, other)?,
        }
    }

    let path = file.unwrap_or_else(|| "-".to_string());
    let mut reader = LineReader::new(open(&path)?);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut printed = 0usize;
    while printed < count {
        let Some(line) = reader.next_line()? else { break };
        out.write_all(line.bytes)?;
        out.write_all(b"\n")?;
        printed += 1;
    }
    out.flush()?;
    Ok(())
}

fn cmd_sample(args: &[String]) -> Result<(), Fail> {
    let mut count = 10usize;
    let mut seed: Option<u64> = None;
    let mut file: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-n" | "--lines" => count = parse_number(need_value(iter.next(), "-n")?, "-n")?,
            "--seed" => seed = Some(parse_number(need_value(iter.next(), "--seed")?, "--seed")?),
            other if is_flag(other) => return Err(unknown_flag(other)),
            other => take_positional(&mut file, other)?,
        }
    }

    let seed = seed.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5EED)
    });
    let path = file.unwrap_or_else(|| "-".to_string());
    let mut reader = LineReader::new(open(&path)?);
    let mut reservoir: Reservoir<Vec<u8>> = Reservoir::new(count, seed);
    while let Some(line) = reader.next_line()? {
        if line.is_blank() {
            continue;
        }
        reservoir.offer(line.number, || line.bytes.to_vec());
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    for (_, bytes) in reservoir.into_sorted() {
        out.write_all(&bytes)?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    Ok(())
}

/// `--json`, `--progress`, `--fail-on-invalid` and the trailing file argument
/// are identical across `stats` and `schema`; parsed together so each command
/// only has to handle the flags it does not share.
struct CommonFlags {
    as_json: bool,
    progress: bool,
    fail_on_invalid: bool,
    file: Option<String>,
}

impl CommonFlags {
    fn new() -> Self {
        CommonFlags {
            as_json: false,
            progress: false,
            fail_on_invalid: false,
            file: None,
        }
    }

    /// Consumes `arg` if it is one of the shared flags. Callers should try
    /// this before matching their own flags and before falling back to
    /// `take_positional`.
    fn try_take(&mut self, arg: &str) -> bool {
        match arg {
            "--json" => self.as_json = true,
            "--progress" => self.progress = true,
            "--fail-on-invalid" => self.fail_on_invalid = true,
            _ => return false,
        }
        true
    }

    fn path(&self) -> &str {
        self.file.as_deref().unwrap_or("-")
    }

    /// Turns `--fail-on-invalid` into an error, once the report has already
    /// been printed. `message` describes what `count` refers to, e.g.
    /// "invalid line(s) found".
    fn check_invalid(&self, count: u64, message: &str) -> Result<(), Fail> {
        if self.fail_on_invalid && count > 0 {
            return Err(Fail::Message(format!(
                "{} {} (--fail-on-invalid)",
                count, message
            )));
        }
        Ok(())
    }
}

fn cmd_stats(args: &[String]) -> Result<(), Fail> {
    let mut options = StatsOptions::default();
    let mut common = CommonFlags::new();
    let mut top = 10usize;
    let mut min_count = 0u64;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if common.try_take(arg) {
            continue;
        }
        match arg.as_str() {
            "--field" | "-f" => {
                let raw = need_value(iter.next(), "--field")?;
                let path = FieldPath::parse(raw)
                    .map_err(|err| Fail::Usage(format!("bad field path '{}': {}", raw, err)))?;
                options.fields.push(path);
            }
            "--top" => top = parse_number(need_value(iter.next(), "--top")?, "--top")?,
            "--min-count" => {
                min_count = parse_number(need_value(iter.next(), "--min-count")?, "--min-count")?;
            }
            "--max-errors" => {
                options.max_issues =
                    parse_number(need_value(iter.next(), "--max-errors")?, "--max-errors")?;
            }
            other if is_flag(other) => return Err(unknown_flag(other)),
            other => take_positional(&mut common.file, other)?,
        }
    }

    let mut reader = LineReader::new(open(common.path())?);
    let mut stats = Stats::new(options);
    let mut ticker = Progress::new(PROGRESS_INTERVAL);
    while let Some(line) = reader.next_line()? {
        stats.observe(&line);
        if common.progress {
            ticker.tick(reader.lines_read(), reader.bytes_read());
        }
    }
    let report = if common.as_json {
        let mut text = stats.report_json(common.path(), min_count);
        text.push('\n');
        text
    } else {
        stats.report_text(common.path(), top, min_count)
    };
    emit(&report)?;
    common.check_invalid(stats.invalid, "invalid line(s) found")
}

fn cmd_schema(args: &[String]) -> Result<(), Fail> {
    let mut options = SchemaOptions::default();
    let mut common = CommonFlags::new();
    let mut min_rate = 0.0f64;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if common.try_take(arg) {
            continue;
        }
        match arg.as_str() {
            "--depth" => options.max_depth = parse_number(need_value(iter.next(), "--depth")?, "--depth")?,
            "--min-rate" => {
                min_rate = parse_number(need_value(iter.next(), "--min-rate")?, "--min-rate")?;
                if !(0.0..=1.0).contains(&min_rate) {
                    return Err(Fail::Usage("--min-rate must be between 0 and 1".to_string()));
                }
            }
            other if is_flag(other) => return Err(unknown_flag(other)),
            other => take_positional(&mut common.file, other)?,
        }
    }
    if options.max_depth == 0 {
        return Err(Fail::Usage("--depth must be at least 1".to_string()));
    }

    let mut reader = LineReader::new(open(common.path())?);
    let mut schema = Schema::new(options);
    let mut ticker = Progress::new(PROGRESS_INTERVAL);
    while let Some(line) = reader.next_line()? {
        if line.is_blank() {
            continue;
        }
        match std::str::from_utf8(line.bytes).ok().map(jsonl_peek::parse) {
            Some(Ok(value)) => schema.observe(&value),
            _ => schema.observe_invalid(),
        }
        if common.progress {
            ticker.tick(reader.lines_read(), reader.bytes_read());
        }
    }

    let report = if common.as_json {
        let mut text = schema.report_json(min_rate);
        text.push('\n');
        text
    } else {
        schema.report_text(min_rate)
    };
    emit(&report)?;
    common.check_invalid(schema.skipped, "unparseable line(s) skipped")
}

#[cfg(test)]
mod tests {
    use super::Progress;

    #[test]
    fn reports_once_per_interval() {
        let mut ticker = Progress::new(10);
        let mut fired_at = Vec::new();
        for lines in 1..=25u64 {
            let next_before = ticker.next;
            ticker.tick(lines, 0);
            if ticker.next != next_before {
                fired_at.push(lines);
            }
        }
        assert_eq!(fired_at, vec![10, 20]);
    }

    #[test]
    fn a_burst_past_several_intervals_reports_once() {
        let mut ticker = Progress::new(10);
        let next_before = ticker.next;
        ticker.tick(45, 0);
        assert_ne!(ticker.next, next_before);
        assert!(ticker.next > 45);
        // The threshold caught up in one jump, not several small ones.
        assert_eq!(ticker.next, 50);
    }
}
