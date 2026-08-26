use std::process::ExitCode;

use console::style;

use crate::ui;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Status {
	Healthy,
	Info,
	Warning,
	Error,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Check {
	label: String,
	value: String,
	status: Status,
	remedy: Option<String>,
}

pub(super) struct Section {
	pub title: &'static str,
	pub debug: bool,
	pub checks: Vec<Check>,
}

pub(super) struct Report {
	pub sections: Vec<Section>,
}

impl Report {
	pub fn status(&self) -> Status {
		self.sections
			.iter()
			.flat_map(|section| &section.checks)
			.map(|check| check.status)
			.max_by_key(|status| status.severity())
			.unwrap_or(Status::Info)
	}

	pub fn print(&self) {
		ui::heading("a365 doctor");
		let status = self.status();
		println!(
			"{}",
			status.paint(format!("{} {}", status.symbol(), status))
		);
		for section in &self.sections {
			if section.debug {
				println!("\n{}", style(section.title).bold().magenta());
			} else {
				ui::heading(section.title);
			}
			let rows =
				section.checks.iter().map(Check::row).collect::<Vec<_>>();
			ui::grid(&rows);
		}
	}

	pub fn exit_code(&self) -> ExitCode {
		if self.status() == Status::Error {
			ExitCode::FAILURE
		} else {
			ExitCode::SUCCESS
		}
	}
}

impl Status {
	fn severity(self) -> u8 {
		match self {
			Self::Info => 0,
			Self::Healthy => 1,
			Self::Warning => 2,
			Self::Error => 3,
		}
	}

	fn symbol(self) -> &'static str {
		match self {
			Self::Healthy => "✓",
			Self::Info => "○",
			Self::Warning => "●",
			Self::Error => "✗",
		}
	}

	fn paint(self, text: impl std::fmt::Display) -> String {
		match self {
			Self::Healthy => style(text).green().bold().to_string(),
			Self::Info => style(text).cyan().to_string(),
			Self::Warning => style(text).yellow().bold().to_string(),
			Self::Error => style(text).red().bold().to_string(),
		}
	}
}

impl std::fmt::Display for Status {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		formatter.write_str(match self {
			Self::Healthy | Self::Info => "Healthy",
			Self::Warning => "Attention needed",
			Self::Error => "Unhealthy",
		})
	}
}

impl Check {
	pub(crate) fn new(
		label: impl Into<String>,
		value: impl Into<String>,
		status: Status,
	) -> Self {
		Self {
			label: label.into(),
			value: value.into(),
			status,
			remedy: None,
		}
	}

	pub(crate) fn remedy(mut self, remedy: impl Into<String>) -> Self {
		self.remedy = Some(remedy.into());
		self
	}

	pub(crate) fn row(&self) -> [String; 2] {
		let value = self.remedy.as_ref().map_or_else(
			|| self.value.clone(),
			|remedy| format!("{} — {remedy}", self.value),
		);
		[
			self.status.paint(format!(
				"{} {}",
				self.status.symbol(),
				self.label
			)),
			value,
		]
	}
}
