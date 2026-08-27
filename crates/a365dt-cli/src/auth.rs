use std::{
	io::{self, IsTerminal},
	process::{Command, Stdio},
};

#[cfg(target_os = "macos")]
use security_framework::{
	item::{ItemClass, ItemSearchOptions, SearchResult},
	passwords::{delete_generic_password, set_generic_password},
};

#[cfg(target_os = "macos")]
use crate::app_files;
use crate::{error::Error, ui};

const ACCESS_TOKEN_URL: &str =
	"https://anime365.ru/api/accessToken?app=app-70510a2eebd4c6a4aa6e4a0e";
const ACCESS_TOKEN_HELP: &str = r#"No Anime365 access token was found and a365 cannot prompt here.

Run a365 in an interactive terminal, or provide the token through the
ANIME365_ACCESS_TOKEN process environment variable."#;
#[cfg(target_os = "macos")]
const KEYCHAIN_ITEM: &str = "anime365-access-token";
#[cfg(target_os = "macos")]
const KEYCHAIN_ACCOUNT: &str = app_files::APPLICATION_ID;
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

pub(crate) enum AccessToken {
	Environment(String),
	#[cfg(target_os = "macos")]
	Keychain(String),
	Browser(String),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Presentation {
	Silent,
	Detailed,
}

impl AccessToken {
	pub(crate) fn value(&self) -> &str {
		match self {
			Self::Environment(token) | Self::Browser(token) => token,
			#[cfg(target_os = "macos")]
			Self::Keychain(token) => token,
		}
	}
}

pub(crate) fn access_token() -> Result<AccessToken, Error> {
	access_token_with(Presentation::Detailed)
}

pub(crate) fn access_token_silently() -> Result<AccessToken, Error> {
	access_token_with(Presentation::Silent)
}

fn access_token_with(presentation: Presentation) -> Result<AccessToken, Error> {
	#[cfg(not(target_os = "macos"))]
	let _ = presentation;
	if let Ok(token) = std::env::var("ANIME365_ACCESS_TOKEN")
		&& !token.trim().is_empty()
	{
		return Ok(AccessToken::Environment(token.trim().to_owned()));
	}
	#[cfg(target_os = "macos")]
	if let Some(token) = keychain_token(presentation) {
		if presentation == Presentation::Detailed {
			ui::note("Using Anime365 access token from macOS Keychain.");
		}
		return Ok(AccessToken::Keychain(token));
	}
	browser_access_token()
}

fn browser_access_token() -> Result<AccessToken, Error> {
	if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
		return Err(Error::new(ACCESS_TOKEN_HELP));
	}
	ui::note(format!("Opening {ACCESS_TOKEN_URL}"));
	if !open_browser(ACCESS_TOKEN_URL) {
		ui::warning("Could not open the browser automatically.");
	}
	ui::note(
		"If Anime365 says authorization is required, sign in and reload the page.",
	);
	let token = ui::secret("Paste access token:")?;
	if token.is_empty() {
		return Err(Error::new("The Anime365 access token cannot be empty."));
	}
	Ok(AccessToken::Browser(token))
}

#[cfg(target_os = "macos")]
fn keychain_token(presentation: Presentation) -> Option<String> {
	if let Some(token) = keychain_token_for(KEYCHAIN_ACCOUNT, presentation) {
		return Some(token);
	}
	let token =
		keychain_token_for(app_files::LEGACY_APPLICATION_ID, presentation)?;
	match set_generic_password(
		KEYCHAIN_ITEM,
		KEYCHAIN_ACCOUNT,
		token.as_bytes(),
	) {
		Ok(()) => {
			let _ = delete_generic_password(
				KEYCHAIN_ITEM,
				app_files::LEGACY_APPLICATION_ID,
			);
			if presentation == Presentation::Detailed {
				ui::note(
					"Migrated the Anime365 token to the a365 Keychain account.",
				);
			}
		}
		Err(error) => {
			if presentation == Presentation::Detailed {
				ui::warning(format!(
					"Could not migrate the Anime365 token in macOS Keychain: {error}"
				));
			}
		}
	}
	Some(token)
}

#[cfg(target_os = "macos")]
fn keychain_token_for(
	account: &str,
	presentation: Presentation,
) -> Option<String> {
	let mut search = ItemSearchOptions::new();
	let result = search
		.class(ItemClass::generic_password())
		.service(KEYCHAIN_ITEM)
		.account(account)
		.load_data(true)
		.search();
	match result {
		Ok(results) => results.into_iter().find_map(|result| match result {
			SearchResult::Data(token) => match String::from_utf8(token) {
				Ok(token) if !token.trim().is_empty() => {
					Some(token.trim().to_owned())
				}
				Ok(_) => None,
				Err(error) => {
					if presentation == Presentation::Detailed {
						ui::warning(format!(
							"Could not read the Anime365 access token from macOS Keychain: {error}"
						));
					}
					None
				}
			},
			SearchResult::Ref(_)
			| SearchResult::Dict(_)
			| SearchResult::Other => None,
		}),
		Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => None,
		Err(error) => {
			if presentation == Presentation::Detailed {
				ui::warning(format!(
					"Could not read the Anime365 access token from macOS Keychain: {error}"
				));
			}
			None
		}
	}
}

#[cfg(target_os = "macos")]
pub(crate) fn store_if_requested(
	access_token: &AccessToken,
) -> Result<(), Error> {
	let AccessToken::Browser(token) = access_token else {
		return Ok(());
	};
	if !ui::confirm("Save this token in macOS Keychain?", true)? {
		return Ok(());
	}
	set_generic_password(KEYCHAIN_ITEM, KEYCHAIN_ACCOUNT, token.as_bytes())
		.map_err(|error| {
			Error::with_debug(
				"Could not save the Anime365 access token in macOS Keychain.",
				error,
			)
		})?;
	ui::success("Saved access token in macOS Keychain.");
	Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn store_if_requested(
	_access_token: &AccessToken,
) -> Result<(), Error> {
	Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn remove_stored_token() -> Result<(), Error> {
	for account in [KEYCHAIN_ACCOUNT, app_files::LEGACY_APPLICATION_ID] {
		match delete_generic_password(KEYCHAIN_ITEM, account) {
			Ok(()) => {}
			Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => {}
			Err(error) => {
				return Err(Error::with_debug(
					"Could not remove the Anime365 access token from macOS Keychain.",
					error,
				));
			}
		}
	}
	Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn remove_stored_token() -> Result<(), Error> {
	Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn open_browser(url: &str) -> bool {
	spawn_browser(Command::new("open").arg(url))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn open_browser(url: &str) -> bool {
	spawn_browser(Command::new("xdg-open").arg(url))
}

#[cfg(not(unix))]
pub(crate) fn open_browser(_url: &str) -> bool {
	false
}

#[cfg(unix)]
fn spawn_browser(command: &mut Command) -> bool {
	command
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.is_ok()
}
