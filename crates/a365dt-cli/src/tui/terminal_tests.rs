use crossterm::event::{
	KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use std::sync::{Arc, Mutex};

use super::{Lifecycle, TerminalControl, key_event, mouse_event};
use crate::tui::{
	state::{Destination, Event, Key},
	view::HitMap,
};

#[test]
fn keyboard_adapter_maps_navigation_text_and_cancellation() {
	assert_eq!(
		[
			key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
			key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
			key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
			key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
		],
		[
			Some(Key::Character('x')),
			Some(Key::Up),
			Some(Key::Quit),
			Some(Key::Quit),
		],
	);
}

#[test]
fn mouse_adapter_maps_navigation_rows_and_wheel() {
	let hit_map = HitMap {
		nav: vec![(Rect::new(0, 0, 10, 2), Destination::Moments)],
		rows: vec![(Rect::new(0, 3, 40, 1), 7)],
	};
	let event = |kind, column, row| MouseEvent {
		kind,
		column,
		row,
		modifiers: KeyModifiers::NONE,
	};

	assert_eq!(
		[
			mouse_event(
				event(MouseEventKind::Down(MouseButton::Left), 1, 1),
				&hit_map,
			),
			mouse_event(
				event(MouseEventKind::Down(MouseButton::Left), 1, 3),
				&hit_map,
			),
			mouse_event(event(MouseEventKind::ScrollUp, 0, 0), &hit_map),
			mouse_event(event(MouseEventKind::ScrollDown, 0, 0), &hit_map),
		],
		[
			Some(Event::ActivateDestination(Destination::Moments)),
			Some(Event::ActivateRow(7)),
			Some(Event::Scroll(-3)),
			Some(Event::Scroll(3)),
		],
	);
}

#[test]
fn lifecycle_adapter_pairs_terminal_entry_with_idempotent_cleanup() {
	struct FakeTerminal(Arc<Mutex<Vec<&'static str>>>);

	impl TerminalControl for FakeTerminal {
		fn enter(&mut self) -> std::io::Result<()> {
			self.0.lock().unwrap().push("enter");
			Ok(())
		}

		fn restore(&mut self) {
			self.0.lock().unwrap().push("restore");
		}
	}

	let calls = Arc::new(Mutex::new(Vec::new()));
	let mut lifecycle =
		Lifecycle::enter(FakeTerminal(Arc::clone(&calls))).unwrap();
	assert_eq!(*calls.lock().unwrap(), vec!["enter"]);

	lifecycle.restore();
	lifecycle.restore();
	drop(lifecycle);

	assert_eq!(*calls.lock().unwrap(), vec!["enter", "restore"]);
}
