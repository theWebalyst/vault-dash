///! Application logic
///!
///! Edit src/custom/app.rs to create a customised fork of logtail-dash
use linemux::MuxedLines;
use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};
use std::fs::File;
use std::io::{Error, ErrorKind, Write};
use structopt::StructOpt;
use tempfile::NamedTempFile;

use crate::custom::opt::Opt;
use crate::shared::util::StatefulList;

pub struct App {
	pub opt: Opt,
	pub dash_state: DashState,
	pub monitors: HashMap<String, LogMonitor>,
	pub logfiles: MuxedLines,
}

impl App {
	pub async fn new() -> Result<App, std::io::Error> {
		let mut opt = Opt::from_args();

		if opt.files.is_empty() {
			println!("{}: no logfile(s) specified.", Opt::clap().get_name());
			println!(
				"Try '{} --help' for more information.",
				Opt::clap().get_name()
			);
			return Err(Error::new(ErrorKind::Other, "missing logfiles"));
		}
		let mut dash_state = DashState::new();
		let mut monitors: HashMap<String, LogMonitor> = HashMap::new();
		let mut logfiles = MuxedLines::new()?;
		let mut parser_output: Option<tempfile::NamedTempFile> = if opt.debug_parser {
			dash_state.main_view = DashViewMain::DashVertical;
			opt.files = opt.files[0..1].to_vec();
			let named_file = NamedTempFile::new()?;
			let path = named_file.path();
			let path_str = path
				.to_str()
				.ok_or_else(|| Error::new(ErrorKind::Other, "invalid path"))?;
			opt.files.push(String::from(path_str));
			Some(named_file)
		} else {
			None
		};
		println!("Loading {} files...", opt.files.len());
		for f in &opt.files {
			println!("file: {}", f);
			let mut monitor = LogMonitor::new(f.to_string(), opt.lines_max);
			if opt.debug_parser && monitor.index == 0 {
				if let Some(named_file) = parser_output {
					monitor.metrics.debug_logfile = Some(named_file);
					parser_output = None;
					dash_state.debug_ui = true;
				}
			}
			if opt.ignore_existing {
				monitors.insert(f.to_string(), monitor);
			} else {
				match monitor.load_logfile() {
					Ok(()) => {
						monitors.insert(f.to_string(), monitor);
					}
					Err(e) => {
						println!("...failed: {}", e);
						return Err(e);
					}
				}
			}
			match logfiles.add_file(&f).await {
				Ok(_) => (),
				Err(e) => {
					println!("ERROR: {}", e);
					println!(
						"Note: it is ok for the file not to exist, but the file's parent directory must exist."
					);
					return Err(e);
				}
			}
		}

		Ok(App {
			opt,
			dash_state,
			monitors,
			logfiles,
		})
	}
}

pub struct LogMonitor {
	pub index: usize,
	pub content: StatefulList<String>,
	pub logfile: String,
	pub metrics: VaultMetrics,
	max_content: usize, // Limit number of lines in content
}

use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT_MONITOR: AtomicUsize = AtomicUsize::new(0);

impl LogMonitor {
	pub fn new(f: String, max_lines: usize) -> LogMonitor {
		let index = NEXT_MONITOR.fetch_add(1, Ordering::Relaxed);
		LogMonitor {
			index,
			logfile: f,
			max_content: max_lines,
			metrics: VaultMetrics::new(),
			content: StatefulList::with_items(vec![]),
		}
	}

	pub fn load_logfile(&mut self) -> std::io::Result<()> {
		use std::io::{BufRead, BufReader};

		let f = File::open(self.logfile.to_string());
		let f = match f {
			Ok(file) => file,
			Err(_e) => return Ok(()), // It's ok for a logfile not to exist yet
		};

		let f = BufReader::new(f);

		for line in f.lines() {
			let line = line.expect("Unable to read line");
			self.process_line(&line)?
		}

		Ok(())
	}

	pub fn process_line(&mut self, line: &str) -> Result<(), std::io::Error> {
		self.metrics.gather_metrics(&line)?;
		self.append_to_content(line) // Show in TUI
	}

	pub fn append_to_content(&mut self, text: &str) -> Result<(), std::io::Error> {
		self.content.items.push(text.to_string());
		if self.content.items.len() > self.max_content {
			self.content.items = self
				.content
				.items
				.split_off(self.content.items.len() - self.max_content);
		}
		Ok(())
	}

	fn _reset_metrics(&mut self) {}
}

use regex::Regex;

lazy_static::lazy_static! {
	// static ref REGEX_ERROR = "The regex failed to compile. This is a bug.";
	static ref LOG_LINE_PATTERN: Regex =
		Regex::new(r"(?P<category>^[A-Z]{4}) (?P<time_string>[^ ]{35}) (?P<source>\[.*\]) (?P<message>.*)").expect("The regex failed to compile. This is a bug.");

	// static ref STATE_PATTERN: Regex =
	//   Regex::new(r"vault.rs .*No. of Elders: (?P<elders>\d+)").expect(REGEX_ERROR);

	// static ref COUNTS_PATTERN: Regex =215

	// Regex::new(r"vault.rs .*No. of Adults: (?P<elders>\d+)").expect(REGEX_ERROR);
}

enum VaultAgebracket {
	Unknown,
	Child,
	Adult,
	Elder,
}

pub struct VaultMetrics {
	pub vault_started: Option<DateTime<FixedOffset>>,
	pub running_message: Option<String>,
	pub running_version: Option<String>,
	pub category_count: HashMap<String, usize>,
	pub timeline: Vec<LogEntry>,
	pub most_recent: Option<DateTime<FixedOffset>>,
	agebracket: VaultAgebracket,
	adults: usize,
	elders: usize,

	pub debug_logfile: Option<NamedTempFile>,
	parser_output: String,
}

impl VaultMetrics {
	fn new() -> VaultMetrics {
		VaultMetrics {
			// Start
			vault_started: None,
			running_message: None,
			running_version: None,

			// Timeline
			timeline: Vec::<LogEntry>::new(),
			most_recent: None,

			// Counts
			category_count: HashMap::new(),

			// State (vault)
			agebracket: VaultAgebracket::Child,

			// State (network)
			adults: 0,
			elders: 0,

			// Debug
			debug_logfile: None,
			parser_output: String::from("-"),
		}
	}

	fn reset_metrics(&mut self) {
		self.agebracket = VaultAgebracket::Child;
		self.adults = 0;
		self.elders = 0;
	}

	///! Process a line from a SAFE Vault logfile.
	///! May add a LogEntry to the VaultMetrics::timeline vector.
	///! Use a created LogEntry to update metrics.
	pub fn gather_metrics(&mut self, line: &str) -> Result<(), std::io::Error> {
		// For debugging LogEntry::decode()
		let mut parser_result = String::from("");
		if let Some(mut entry) = LogEntry::decode(line).or_else(|| self.parse_start(line)) {
			if entry.time.is_none() {
				entry.time = self.most_recent;
			} else {
				self.most_recent = entry.time;
			}

			self.parser_output = entry.parser_output.clone();
			self.parse_states(&entry); // May overwrite self.parser_output
			parser_result = self.parser_output.clone();
			self.timeline.push(entry);

			// TODO Trim timeline
		}

		// --debug-parser - prints parser results for a single logfile
		// to a temp logfile which is displayed in the adjacent window.
		match &self.debug_logfile {
			Some(f) => {
				use std::io::Seek;
				let mut file = f.reopen()?;
				file.seek(std::io::SeekFrom::End(0))?;
				writeln!(file, "{}", &parser_result)?
			}
			None => (),
		};
		Ok(())
	}

	///! Returm a LogEntry and capture metadata for logfile vault start:
	///!    'Running safe-vault v0.24.0'
	pub fn parse_start(&mut self, line: &str) -> Option<LogEntry> {
		let running_prefix = String::from("Running safe-vault ");

		if line.starts_with(&running_prefix) {
			self.running_message = Some(line.to_string());
			self.running_version = Some(line[running_prefix.len()..].to_string());
			self.vault_started = self.most_recent;
			let parser_output = format!(
				"START at {}",
				self
					.most_recent
					.map_or(String::from("None"), |m| format!("{}", m))
			);

			return Some(LogEntry {
				logstring: String::from(line),
				category: String::from("START"),
				time: self.most_recent,
				source: String::from(""),
				message: line.to_string(),
				parser_output,
			});
		}

		None
	}

	///! Capture state updates and return true if metrics were updated
	pub fn parse_states(&mut self, entry: &LogEntry) -> bool {
		let mut updated = false;
		let re = Regex::new(
			r"^.*vault\.rs.*No. of Elders: (?P<elders>\d+)|(?x)
      (?-x)^.*vault\.rs.*No. of Adults: (?P<adults>\d+)|(?x)
      (?-x)^.*vault\.rs.*Initializing new Vault as (?P<initas>[[:alpha:]]+)|(?x)
      (?-x)^.*vault\.rs.*Vault promoted to (?P<promoteto>[[:alpha:]]+)(?x)",
		)
		.expect("Woops"); // TODO: make the expression a static (see LOG_LINE_PATTERN)

		if let Some(captures) = re.captures(entry.logstring.as_str()) {
			let elders = captures.name("elders").map_or("", |m| m.as_str());
			if !elders.is_empty() {
				self.parser_output = format!("ELDERS: {}", elders);
				updated = true
			} else {
				let adults = captures.name("adults").map_or("", |m| m.as_str());
				if !adults.is_empty() {
					self.parser_output = format!("ADULTS: {}", adults);
					updated = true
				} else {
					let agebracket = captures
						.name("initas")
						.or_else(|| captures.name("promoteto"))
						.map_or("", |m| m.as_str());
					self.parser_output = format!("Vault agebracket: {}", agebracket);
					if !agebracket.is_empty() {
						self.parser_output = format!("Vault agebracket: {}", agebracket);
						self.agebracket = match agebracket {
							"Child" => VaultAgebracket::Child,
							"Adult" => VaultAgebracket::Adult,
							"Elder" => VaultAgebracket::Elder,
							_ => VaultAgebracket::Unknown,
						};
						updated = true
					}
				}
			}
		}
		updated
	}

	///! TODO
	pub fn parse_counts(&mut self, entry: &LogEntry) {}
}

///! Decoded logfile entries for a vault timeline metric
pub struct LogEntry {
	pub logstring: String,
	pub category: String, // First word, "Running", "INFO", "WARN" etc
	pub time: Option<DateTime<FixedOffset>>,
	pub source: String,
	pub message: String,

	pub parser_output: String,
}

impl LogEntry {
	///! Decode vault logfile lines of the form:
	///!    INFO 2020-07-08T19:58:26.841778689+01:00 [src/bin/safe_vault.rs:114]
	///!    WARN 2020-07-08T19:59:18.540118366+01:00 [src/data_handler/idata_handler.rs:744] 552f45..: Failed to get holders metadata from DB
	///!
	pub fn decode(line: &str) -> Option<LogEntry> {
		let mut test_entry = LogEntry {
			logstring: String::from(line),
			category: String::from("test"),
			time: None,
			source: String::from(""),
			message: String::from(""),
			parser_output: String::from("decode()..."),
		};

		if line.is_empty() {
			return None;
		}

		LogEntry::parse_info_line(line)
	}

	///! Parse a line of the form:
	///!    INFO 2020-07-08T19:58:26.841778689+01:00 [src/bin/safe_vault.rs:114]
	///!    WARN 2020-07-08T19:59:18.540118366+01:00 [src/data_handler/idata_handler.rs:744] 552f45..: Failed to get holders metadata from DB
	fn parse_info_line(line: &str) -> Option<LogEntry> {
		let captures = LOG_LINE_PATTERN.captures(line)?;

		let category = captures.name("category").map_or("", |m| m.as_str());
		let time_string = captures.name("time_string").map_or("", |m| m.as_str());
		let source = captures.name("source").map_or("", |m| m.as_str());
		let message = captures.name("message").map_or("", |m| m.as_str());
		let mut time_str = String::from("None");
		let time = match DateTime::<FixedOffset>::parse_from_rfc3339(time_string) {
			Ok(time) => {
				time_str = format!("{}", time);
				Some(time)
			}
			Err(e) => None,
		};
		let parser_output = format!(
			"c: {}, t: {}, s: {}, m: {}",
			category, time_str, source, message
		);

		Some(LogEntry {
			logstring: String::from(line),
			category: String::from(category),
			time: time,
			source: String::from(source),
			message: String::from(message),
			parser_output,
		})
	}
}

pub enum DashViewMain {
	DashHorizontal,
	DashVertical,
	DashDebug,
}

pub struct DashState {
	pub main_view: DashViewMain,
	pub debug_ui: bool,

	// For DashViewMain::DashVertical
	dash_vertical: DashVertical,
}

impl DashState {
	pub fn new() -> DashState {
		DashState {
			main_view: DashViewMain::DashHorizontal,
			dash_vertical: DashVertical::new(),
			debug_ui: false,
		}
	}
}

pub struct DashVertical {
	active_view: usize,
}

impl DashVertical {
	pub fn new() -> Self {
		DashVertical { active_view: 0 }
	}
}
