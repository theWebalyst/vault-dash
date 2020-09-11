//! This app monitors and logfiles and displays status in the terminal
//!
//! It is based on logtail-dash, which is a basic logfile dashboard
//! and also a framework for similar apps with customised dahsboard
//! displays.
//!
//! Custom apps based on logtail can be created by creating a
//! fork of logtail-dash and modifying the files in src/custom
//!
//! See README for more information.

#![recursion_limit = "512"] // Prevent select! macro blowing up

///! forks of logterm customise the files in src/custom
#[path = "../custom/mod.rs"]
pub mod custom;
use self::custom::app::{App, DashViewMain};
use self::custom::opt::Opt;
use self::custom::ui::draw_dashboard;

#[macro_use]
extern crate log;
extern crate env_logger;

///! logtail and its forks share code in src/
#[path = "../mod.rs"]
pub mod shared;

use crossterm::{
	event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode},
	execute,
	terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use std::{
	error::Error,
	io::{stdout, Write},
	sync::mpsc,
	thread,
	time::{Duration, Instant},
};

use tui::{
	backend::CrosstermBackend,
	layout::{Constraint, Corner, Direction, Layout},
	style::{Color, Modifier, Style},
	text::{Span, Spans, Text},
	widgets::{Block, BorderType, Borders, List, ListItem, Widget},
	Frame, Terminal,
};

use futures::{
	future::FutureExt, // for `.fuse()`
	pin_mut,
	select,
};

enum Event<I> {
	Input(I),
	Tick,
}

use tokio::stream::StreamExt;

// RUSTFLAGS="-A unused" cargo run --bin logtail-crossterm --features="crossterm" /var/log/auth.log /var/log/dmesg
#[tokio::main]
pub async fn main() -> Result<(), Box<dyn Error>> {
	env_logger::init();
	info!("Started");

	let mut app = match App::new().await {
		Ok(app) => app,
		Err(e) => return Ok(()),
	};

	// Terminal initialization
	enable_raw_mode()?;
	let mut stdout = stdout();
	execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
	let backend = CrosstermBackend::new(stdout);
	let mut terminal = Terminal::new(backend)?;
	let rx = initialise_events(app.opt.tick_rate);
	terminal.clear()?;

	// Use futures of async functions to handle events
	// concurrently with logfile changes.
	loop {
		terminal.draw(|f| draw_dashboard(f, &mut app.dash_state, &mut app.monitors))?;
		let logfiles_future = app.logfiles.next().fuse();
		let events_future = next_event(&rx).fuse();
		pin_mut!(logfiles_future, events_future);

		select! {
			(e) = events_future => {
			match e {
				Ok(Event::Input(event)) => {
					match event.code {
						// For debugging, ~ sends a line to the debug_window
						KeyCode::Char('~') => app.dash_state._debug_window(format!("Event::Input({:#?})", event).as_str()),

						KeyCode::Char('q')|
						KeyCode::Char('Q') => {
							disable_raw_mode()?;
							execute!(
								terminal.backend_mut(),
								LeaveAlternateScreen,
								DisableMouseCapture
							)?;
							terminal.show_cursor()?;
							break Ok(());
						},
						KeyCode::Char('h')|
						KeyCode::Char('H') => app.dash_state.main_view = DashViewMain::DashHorizontal,
						KeyCode::Char('v')|
						KeyCode::Char('V') => app.dash_state.main_view = DashViewMain::DashVertical,
						KeyCode::Char('D') => app.dash_state.main_view = DashViewMain::DashDebug,
						KeyCode::Down => app.handle_arrow_down(),
						KeyCode::Up => app.handle_arrow_up(),
						KeyCode::Right|
						KeyCode::Tab => app.change_focus_next(),
						KeyCode::Left => app.change_focus_previous(),
						_ => {}
					}
				}

				Ok(Event::Tick) => {
				// draw_dashboard(&mut f, &dash_state, &mut monitors).unwrap();
				// draw_dashboard(f, &dash_state, &mut monitors)?;
				}

				Err(error) => {
				println!("{}", error);
				}
			}
			},

			(line) = logfiles_future => {
			match line {
				Some(Ok(line)) => {
					trace!("logfiles_future line");
					app.dash_state._debug_window(format!("logfile: {}", line.line()).as_str());
					let source_str = line.source().to_str().unwrap();
					let source = String::from(source_str);

					match app.monitors.get_mut(&source) {
						Some(monitor) => monitor.append_to_content(line.line())?,
						None => (),
					}
				},
				Some(Err(e)) => {
					app.dash_state._debug_window(format!("logfile error: {:#?}", e).as_str());
					panic!("{}", e)
				}
				None => (),
			}
			},
		}
	}
}
// type Tx = std::sync::mpsc::Sender<Event<crossterm::event::KeyEvent>>;
type Rx = std::sync::mpsc::Receiver<Event<crossterm::event::KeyEvent>>;

fn initialise_events(tick_rate: u64) -> Rx {
	let tick_rate = Duration::from_millis(tick_rate);
	let (tx, rx) = mpsc::channel(); // Setup input handling

	thread::spawn(move || {
		let mut last_tick = Instant::now();
		loop {
			// poll for tick rate duration, if no events, sent tick event.
			if event::poll(tick_rate - last_tick.elapsed()).unwrap() {
				if let CEvent::Key(key) = event::read().unwrap() {
					tx.send(Event::Input(key)).unwrap();
				}
			}
			if last_tick.elapsed() >= tick_rate {
				tx.send(Event::Tick).unwrap(); // <-- PANICS HERE
				last_tick = Instant::now();
			}

			if last_tick.elapsed() >= tick_rate {
				match tx.send(Event::Tick) {
					Ok(()) => last_tick = Instant::now(),
					Err(e) => println!("send error: {}", e),
				}
			}
		}
	});
	rx
}

async fn next_event(rx: &Rx) -> Result<Event<crossterm::event::KeyEvent>, mpsc::RecvError> {
	rx.recv()
}
