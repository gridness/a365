use ratatui::{
	Frame,
	layout::{Constraint, Direction, Layout, Rect},
	style::{Color, Modifier, Style},
	text::{Line, Span},
	widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use super::state::{App, Destination};

const ACCENT: Color = Color::Rgb(245, 166, 35);
const MUTED: Color = Color::Rgb(135, 139, 148);

#[derive(Default)]
pub(crate) struct HitMap {
	pub nav: Vec<(Rect, Destination)>,
	pub rows: Vec<(Rect, usize)>,
}

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut App) -> HitMap {
	let area = frame.area();
	let has_filter = app.shows_filter();
	let constraints = if has_filter {
		vec![
			Constraint::Length(3),
			Constraint::Length(3),
			Constraint::Min(4),
			Constraint::Length(2),
		]
	} else {
		vec![
			Constraint::Length(3),
			Constraint::Min(4),
			Constraint::Length(2),
		]
	};
	let rows = Layout::default()
		.direction(Direction::Vertical)
		.constraints(constraints)
		.split(area);
	let mut map = HitMap::default();
	render_navigation(frame, rows[0], app.destination, &mut map);
	let body = if has_filter {
		render_filter(frame, rows[1], app);
		rows[2]
	} else {
		rows[1]
	};
	render_body(frame, body, app, &mut map);
	let help = if app.destination == Destination::Search {
		"Type to filter  ·  ↑↓/wheel select  ·  Enter/click open  ·  Tab switch  ·  Esc clear/quit"
	} else if app.destination == Destination::AniList {
		"/ filter  ·  ↑↓/wheel select  ·  Enter/click open  ·  Tab switch  ·  Esc clear/quit"
	} else {
		"↑↓/wheel select  ·  Enter/click open  ·  ←→/Tab switch  ·  Ctrl-Q quit"
	};
	frame.render_widget(
		Paragraph::new(help).style(Style::default().fg(MUTED)),
		*rows.last().expect("the TUI always has a help row"),
	);
	map
}

fn render_navigation(
	frame: &mut Frame<'_>,
	area: Rect,
	active: Destination,
	map: &mut HitMap,
) {
	let cells = Layout::default()
		.direction(Direction::Horizontal)
		.constraints([Constraint::Ratio(1, 6); 6])
		.split(area);
	for (destination, cell) in Destination::ALL.into_iter().zip(cells.iter()) {
		let active_style = Style::default()
			.fg(Color::Black)
			.bg(ACCENT)
			.add_modifier(Modifier::BOLD);
		let style = if destination == active {
			active_style
		} else {
			Style::default().fg(Color::White)
		};
		frame.render_widget(
			Paragraph::new(destination.name())
				.centered()
				.block(Block::default().borders(Borders::ALL))
				.style(style),
			*cell,
		);
		map.nav.push((*cell, destination));
	}
}

fn render_filter(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let query = app.filter_value();
	let placeholder = if app.destination == Destination::AniList {
		"Press / to filter title, list, or status…"
	} else {
		"Search title…"
	};
	let value = if query.is_empty() {
		Line::from(Span::styled(placeholder, Style::default().fg(MUTED)))
	} else {
		Line::from(query.to_owned())
	};
	frame.render_widget(
		Paragraph::new(value).block(
			Block::default()
				.borders(Borders::ALL)
				.border_style(Style::default().fg(ACCENT)),
		),
		area,
	);
}

fn render_body(
	frame: &mut Frame<'_>,
	area: Rect,
	app: &mut App,
	map: &mut HitMap,
) {
	if let Some(profile) = app.profile() {
		let mut lines = vec![
			Line::from(format!(
				"Signed in  {}",
				if profile.documented.is_logined {
					"yes"
				} else {
					"no"
				}
			)),
			Line::from(vec![
				Span::styled("Account  ", Style::default().fg(MUTED)),
				Span::raw(format!(
					"{}{}",
					profile.documented.name.as_deref().unwrap_or("Unknown"),
					profile
						.documented
						.id
						.map_or_else(String::new, |id| format!(" · ID {id}")),
				)),
			]),
			Line::from(format!(
				"Premium  {}{}",
				if profile.documented.is_premium {
					"yes"
				} else {
					"no"
				},
				profile
					.documented
					.premium_until
					.as_deref()
					.map_or_else(String::new, |until| format!(
						" · until {until}"
					)),
			)),
			Line::from(""),
		];
		match &profile.enrichment {
			Ok(enrichment) => {
				if let Some(avatar) = &enrichment.avatar_url {
					lines.push(Line::from(format!("Avatar  {avatar}")));
				}
				lines.push(Line::from(format!(
					"Public Anime365 lists · {} entries",
					enrichment.lists.len()
				)));
				for entry in enrichment.lists.iter().take(30) {
					let progress = entry
						.progress
						.as_deref()
						.map_or_else(String::new, |value| {
							format!(" · {value}")
						});
					let score = entry
						.score
						.as_deref()
						.map_or_else(String::new, |value| {
							format!(" · score {value}")
						});
					lines.push(Line::from(format!(
						"  {:?} · {}{progress}{score}",
						entry.status, entry.title,
					)));
				}
				lines.push(Line::from(format!(
					"Published Moments · {}",
					enrichment.moments.len()
				)));
				for moment in enrichment.moments.iter().take(10) {
					let duration = if moment.duration.is_empty() {
						String::new()
					} else {
						format!(" · {}", moment.duration)
					};
					lines.push(Line::from(format!(
						"  {}{duration}",
						moment.title,
					)));
				}
			}
			Err(error) => lines.push(Line::from(Span::styled(
				format!("Public profile enrichment unavailable · {error}"),
				Style::default().fg(MUTED),
			))),
		}
		frame.render_widget(
			Paragraph::new(lines)
				.wrap(Wrap { trim: true })
				.block(body_block("Anime365 profile")),
			area,
		);
		return;
	}
	if let Some(message) = app.surface_message() {
		frame.render_widget(
			Paragraph::new(message)
				.centered()
				.style(Style::default().fg(MUTED))
				.block(body_block(app.destination.name())),
			area,
		);
		return;
	}
	let title = app.anilist_viewer_name().map_or_else(
		|| app.destination.name().to_owned(),
		|name| format!("AniList · {name}"),
	);
	let items = app
		.items()
		.iter()
		.map(|item| {
			ListItem::new(Line::from(vec![
				Span::styled(
					item.label(),
					Style::default().add_modifier(Modifier::BOLD),
				),
				Span::styled(
					format!("  {}", item.detail()),
					Style::default().fg(MUTED),
				),
			]))
		})
		.collect::<Vec<_>>();
	let mut list_state = ListState::default()
		.with_offset(app.viewport)
		.with_selected((!items.is_empty()).then_some(app.selected));
	let list = List::new(items)
		.block(body_block(&title))
		.highlight_symbol("› ")
		.highlight_style(
			Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
		);
	frame.render_stateful_widget(list, area, &mut list_state);
	app.viewport = list_state.offset();
	let inner = body_block("").inner(area);
	for visible in 0..usize::from(inner.height) {
		let index = app.viewport + visible;
		if index >= app.items().len() {
			break;
		}
		map.rows.push((
			Rect::new(inner.x, inner.y + visible as u16, inner.width, 1),
			index,
		));
	}
}

fn body_block(title: &str) -> Block<'_> {
	Block::default()
		.borders(Borders::ALL)
		.title(format!(" {title} "))
		.border_style(Style::default().fg(Color::DarkGray))
}
