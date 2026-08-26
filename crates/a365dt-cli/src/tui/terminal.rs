use std::{
	io::{self, Stdout, stdout},
	panic,
	time::Duration,
};

use crossterm::{
	cursor,
	event::{
		self, DisableMouseCapture, EnableMouseCapture, Event as CrosstermEvent,
		KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
		MouseEventKind,
	},
	execute,
	terminal::{
		EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
		enable_raw_mode,
	},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Position};

use super::{
	state::{Event, Key},
	view::HitMap,
};
use crate::error::Error;

type PanicHook = Box<dyn Fn(&panic::PanicHookInfo<'_>) + Send + Sync + 'static>;

pub(crate) struct Session {
	terminal: Terminal<CrosstermBackend<Stdout>>,
	_lifecycle: Lifecycle<SystemTerminal>,
	previous_hook: Option<PanicHook>,
}

/// Owns terminal entry and restoration so every adapter implementation can
/// guarantee that a successful entry is paired with cleanup on drop.
trait TerminalControl {
	fn enter(&mut self) -> io::Result<()>;
	fn restore(&mut self);
}

struct SystemTerminal;

struct Lifecycle<C: TerminalControl> {
	control: C,
	active: bool,
}

impl<C: TerminalControl> Lifecycle<C> {
	fn enter(mut control: C) -> io::Result<Self> {
		control.enter()?;
		Ok(Self {
			control,
			active: true,
		})
	}

	fn restore(&mut self) {
		if self.active {
			self.control.restore();
			self.active = false;
		}
	}
}

impl<C: TerminalControl> Drop for Lifecycle<C> {
	fn drop(&mut self) {
		self.restore();
	}
}

impl TerminalControl for SystemTerminal {
	fn enter(&mut self) -> io::Result<()> {
		enable_raw_mode()?;
		if let Err(error) = execute!(
			stdout(),
			EnterAlternateScreen,
			EnableMouseCapture,
			cursor::Hide
		) {
			let _ = disable_raw_mode();
			return Err(error);
		}
		Ok(())
	}

	fn restore(&mut self) {
		restore_system_terminal();
	}
}

impl Session {
	pub(crate) fn enter() -> Result<Self, Error> {
		let lifecycle =
			Lifecycle::enter(SystemTerminal).map_err(terminal_error)?;
		let previous_hook = panic::take_hook();
		panic::set_hook(Box::new(|info| {
			restore_system_terminal();
			eprintln!("{info}");
		}));
		let terminal = match Terminal::new(CrosstermBackend::new(stdout())) {
			Ok(terminal) => terminal,
			Err(error) => {
				panic::set_hook(previous_hook);
				return Err(terminal_error(error));
			}
		};
		Ok(Self {
			terminal,
			_lifecycle: lifecycle,
			previous_hook: Some(previous_hook),
		})
	}

	pub(crate) fn draw(
		&mut self,
		app: &mut super::state::App,
	) -> Result<HitMap, Error> {
		let mut hit_map = None;
		self.terminal
			.draw(|frame| {
				hit_map = Some(super::view::render(frame, app));
			})
			.map_err(terminal_error)?;
		Ok(hit_map.expect("drawing always creates a hit map"))
	}

	pub(crate) fn event(
		&self,
		hit_map: &HitMap,
	) -> Result<Option<Event>, Error> {
		if !event::poll(Duration::from_millis(100)).map_err(terminal_error)? {
			return Ok(None);
		}
		match event::read().map_err(terminal_error)? {
			CrosstermEvent::Key(key)
				if matches!(
					key.kind,
					KeyEventKind::Press | KeyEventKind::Repeat
				) =>
			{
				Ok(key_event(key).map(Event::Key))
			}
			CrosstermEvent::Mouse(mouse) => Ok(mouse_event(mouse, hit_map)),
			CrosstermEvent::Resize(width, height) => {
				Ok(Some(Event::Resize { width, height }))
			}
			CrosstermEvent::FocusGained
			| CrosstermEvent::FocusLost
			| CrosstermEvent::Paste(_)
			| CrosstermEvent::Key(_) => Ok(None),
		}
	}
}

impl Drop for Session {
	fn drop(&mut self) {
		if let Some(previous_hook) = self.previous_hook.take() {
			panic::set_hook(previous_hook);
		}
	}
}

fn key_event(event: KeyEvent) -> Option<Key> {
	if event.modifiers.contains(KeyModifiers::CONTROL)
		&& matches!(event.code, KeyCode::Char('c' | 'q'))
	{
		return Some(Key::Quit);
	}
	match event.code {
		KeyCode::Char(character)
			if !event.modifiers.intersects(
				KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
			) =>
		{
			Some(Key::Character(character))
		}
		KeyCode::Backspace => Some(Key::Backspace),
		KeyCode::Enter => Some(Key::Enter),
		KeyCode::Esc => Some(Key::Escape),
		KeyCode::Up => Some(Key::Up),
		KeyCode::Down => Some(Key::Down),
		KeyCode::Left => Some(Key::Left),
		KeyCode::Right => Some(Key::Right),
		KeyCode::Home => Some(Key::Home),
		KeyCode::End => Some(Key::End),
		KeyCode::Tab => Some(Key::Tab),
		KeyCode::BackTab => Some(Key::BackTab),
		KeyCode::Char(_)
		| KeyCode::F(_)
		| KeyCode::Null
		| KeyCode::Insert
		| KeyCode::Delete
		| KeyCode::PageUp
		| KeyCode::PageDown
		| KeyCode::CapsLock
		| KeyCode::ScrollLock
		| KeyCode::NumLock
		| KeyCode::PrintScreen
		| KeyCode::Pause
		| KeyCode::Menu
		| KeyCode::KeypadBegin
		| KeyCode::Media(_)
		| KeyCode::Modifier(_) => None,
	}
}

fn mouse_event(event: MouseEvent, hit_map: &HitMap) -> Option<Event> {
	match event.kind {
		MouseEventKind::ScrollUp => Some(Event::Scroll(-3)),
		MouseEventKind::ScrollDown => Some(Event::Scroll(3)),
		MouseEventKind::Down(MouseButton::Left) => {
			let position = Position::new(event.column, event.row);
			hit_map
				.nav
				.iter()
				.find(|(area, _)| area.contains(position))
				.map(|(_, destination)| {
					Event::ActivateDestination(*destination)
				})
				.or_else(|| {
					hit_map
						.rows
						.iter()
						.find(|(area, _)| area.contains(position))
						.map(|(_, row)| Event::ActivateRow(*row))
				})
		}
		MouseEventKind::Down(_)
		| MouseEventKind::Up(_)
		| MouseEventKind::Drag(_)
		| MouseEventKind::Moved
		| MouseEventKind::ScrollLeft
		| MouseEventKind::ScrollRight => None,
	}
}

fn restore_system_terminal() {
	let _ = disable_raw_mode();
	let _ = execute!(
		stdout(),
		LeaveAlternateScreen,
		DisableMouseCapture,
		cursor::Show
	);
}

fn terminal_error(error: io::Error) -> Error {
	Error::with_debug("The full-screen terminal interface failed.", error)
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
