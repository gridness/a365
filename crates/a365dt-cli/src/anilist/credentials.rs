#[cfg(not(target_os = "macos"))]
use std::process::{Command, Stdio};

#[cfg(target_os = "macos")]
use security_framework::{
	item::{ItemClass, ItemSearchOptions, SearchResult},
	passwords::{delete_generic_password, set_generic_password},
};

use crate::{app_files, error::Error};

const ENVIRONMENT_TOKEN: &str = "ANILIST_ACCESS_TOKEN";
const SERVICE: &str = "anilist-access-token";
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Origin {
	Environment,
	SecureStore,
}

pub(super) struct AccessToken {
	value: String,
	pub(super) origin: Origin,
}

/// Persists AniList tokens in an operating-system credential store.
///
/// Implementations must never place token contents in errors, logs, or
/// ordinary application files.
pub(super) trait CredentialStore {
	fn load(&self) -> Result<Option<String>, Error>;
	fn store(&self, token: &str) -> Result<(), Error>;
	fn remove(&self) -> Result<(), Error>;
}

pub(super) struct SystemStore;

impl AccessToken {
	pub(super) fn value(&self) -> &str {
		&self.value
	}
}

pub(super) fn load() -> Result<Option<AccessToken>, Error> {
	let environment = std::env::var(ENVIRONMENT_TOKEN).ok();
	load_with(environment.as_deref(), &SystemStore)
}

pub(super) fn store(token: &str) -> Result<(), Error> {
	SystemStore.store(token)
}

pub(super) fn remove() -> Result<(), Error> {
	SystemStore.remove()
}

fn load_with(
	environment: Option<&str>,
	store: &impl CredentialStore,
) -> Result<Option<AccessToken>, Error> {
	if let Some(token) =
		environment.map(str::trim).filter(|token| !token.is_empty())
	{
		return Ok(Some(AccessToken {
			value: token.to_owned(),
			origin: Origin::Environment,
		}));
	}
	Ok(store.load()?.map(|token| AccessToken {
		value: token,
		origin: Origin::SecureStore,
	}))
}

#[cfg(target_os = "macos")]
impl CredentialStore for SystemStore {
	fn load(&self) -> Result<Option<String>, Error> {
		if let Some(token) = macos_load(app_files::APPLICATION_ID)? {
			return Ok(Some(token));
		}
		let Some(token) = macos_load(app_files::LEGACY_APPLICATION_ID)? else {
			return Ok(None);
		};
		self.store(&token)?;
		macos_remove(app_files::LEGACY_APPLICATION_ID)?;
		Ok(Some(token))
	}

	fn store(&self, token: &str) -> Result<(), Error> {
		set_generic_password(
			SERVICE,
			app_files::APPLICATION_ID,
			token.as_bytes(),
		)
		.map_err(|error| {
			Error::with_debug(
				"Could not save the AniList token in macOS Keychain.",
				error,
			)
		})
	}

	fn remove(&self) -> Result<(), Error> {
		for account in
			[app_files::APPLICATION_ID, app_files::LEGACY_APPLICATION_ID]
		{
			macos_remove(account)?;
		}
		Ok(())
	}
}

#[cfg(target_os = "macos")]
fn macos_load(account: &str) -> Result<Option<String>, Error> {
	let mut search = ItemSearchOptions::new();
	match search
		.class(ItemClass::generic_password())
		.service(SERVICE)
		.account(account)
		.load_data(true)
		.search()
	{
		Ok(results) => results
			.into_iter()
			.find_map(|result| match result {
				SearchResult::Data(value) => Some(value),
				SearchResult::Ref(_)
				| SearchResult::Dict(_)
				| SearchResult::Other => None,
			})
			.map(decode)
			.transpose(),
		Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
		Err(error) => Err(Error::with_debug(
			"Could not read the AniList token from macOS Keychain.",
			error,
		)),
	}
}

#[cfg(target_os = "macos")]
fn macos_remove(account: &str) -> Result<(), Error> {
	match delete_generic_password(SERVICE, account) {
		Ok(()) => Ok(()),
		Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
		Err(error) => Err(Error::with_debug(
			"Could not remove the AniList token from macOS Keychain.",
			error,
		)),
	}
}

#[cfg(target_os = "macos")]
fn decode(value: Vec<u8>) -> Result<String, Error> {
	String::from_utf8(value)
		.map(|value| value.trim().to_owned())
		.map_err(|error| {
			Error::with_debug(
				"The AniList token in macOS Keychain is unreadable.",
				error,
			)
		})
}

#[cfg(not(target_os = "macos"))]
impl CredentialStore for SystemStore {
	fn load(&self) -> Result<Option<String>, Error> {
		if let Some(token) = secret_lookup(app_files::APPLICATION_ID)? {
			return Ok(Some(token));
		}
		let Some(token) = secret_lookup(app_files::LEGACY_APPLICATION_ID)?
		else {
			return Ok(None);
		};
		self.store(&token)?;
		secret_clear(app_files::LEGACY_APPLICATION_ID)?;
		Ok(Some(token))
	}

	fn store(&self, token: &str) -> Result<(), Error> {
		use std::io::Write;

		let mut child = secret_tool()
			.args([
				"store",
				"--label",
				"a365 AniList access token",
				"application",
				app_files::APPLICATION_ID,
				"credential",
				SERVICE,
			])
			.stdin(Stdio::piped())
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.spawn()
			.map_err(secret_service_error)?;
		child
			.stdin
			.take()
			.ok_or_else(|| Error::new("Could not open Linux Secret Service."))?
			.write_all(token.as_bytes())
			.map_err(secret_service_error)?;
		let status = child.wait().map_err(secret_service_error)?;
		if status.success() {
			Ok(())
		} else {
			Err(Error::new(
				"Linux Secret Service refused the AniList token. Unlock the keyring and retry.",
			))
		}
	}

	fn remove(&self) -> Result<(), Error> {
		for account in
			[app_files::APPLICATION_ID, app_files::LEGACY_APPLICATION_ID]
		{
			secret_clear(account)?;
		}
		Ok(())
	}
}

#[cfg(not(target_os = "macos"))]
fn secret_lookup(account: &str) -> Result<Option<String>, Error> {
	let output = secret_tool()
		.args(["lookup", "application", account, "credential", SERVICE])
		.output()
		.map_err(secret_service_error)?;
	if !output.status.success() {
		return Ok(None);
	}
	let token = String::from_utf8(output.stdout)
		.map_err(secret_service_error)?
		.trim()
		.to_owned();
	Ok((!token.is_empty()).then_some(token))
}

#[cfg(not(target_os = "macos"))]
fn secret_clear(account: &str) -> Result<(), Error> {
	let status = secret_tool()
		.args(["clear", "application", account, "credential", SERVICE])
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.status()
		.map_err(secret_service_error)?;
	if status.success() || status.code() == Some(1) {
		Ok(())
	} else {
		Err(Error::new(
			"Linux Secret Service could not remove the AniList token.",
		))
	}
}

#[cfg(not(target_os = "macos"))]
fn secret_tool() -> Command {
	Command::new("secret-tool")
}

#[cfg(not(target_os = "macos"))]
fn secret_service_error(error: impl std::error::Error) -> Error {
	Error::with_debug(
		"Could not use Linux Secret Service. Install libsecret's secret-tool and unlock a keyring.",
		error,
	)
}

#[cfg(test)]
pub(super) mod tests {
	use std::sync::Mutex;

	use pretty_assertions::assert_eq;

	use super::{AccessToken, CredentialStore, Origin, load_with};
	use crate::error::Error;

	#[derive(Default)]
	struct MemoryStore {
		value: Mutex<Option<String>>,
	}

	impl CredentialStore for MemoryStore {
		fn load(&self) -> Result<Option<String>, Error> {
			Ok(self.value.lock().unwrap().clone())
		}

		fn store(&self, token: &str) -> Result<(), Error> {
			*self.value.lock().unwrap() = Some(token.to_owned());
			Ok(())
		}

		fn remove(&self) -> Result<(), Error> {
			*self.value.lock().unwrap() = None;
			Ok(())
		}
	}

	#[test]
	fn environment_fallback_precedes_the_injected_secure_store() {
		let store = MemoryStore::default();
		store.store("stored").unwrap();

		let AccessToken { value, origin } =
			load_with(Some(" environment "), &store).unwrap().unwrap();

		assert_eq!(
			(value, origin),
			("environment".into(), Origin::Environment)
		);
	}

	#[test]
	fn injected_store_round_trips_without_process_environment() {
		let store = MemoryStore::default();
		store.store("secret").unwrap();
		let loaded = load_with(None, &store).unwrap().unwrap();
		store.remove().unwrap();

		assert_eq!(
			(
				loaded.value,
				loaded.origin,
				load_with(None, &store).unwrap().is_none()
			),
			("secret".into(), Origin::SecureStore, true),
		);
	}
}
