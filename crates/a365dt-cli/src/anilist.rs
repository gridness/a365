use std::{io::IsTerminal, time::Duration};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{error::Error, preferences, ui};

mod credentials;
mod oauth;
mod timetable;
mod trending;

pub(crate) use timetable::{ScheduleEntry, current_week};
pub(crate) use trending::{TrendingSeries, trending_series};

const GRAPHQL_ENDPOINT: &str = "https://graphql.anilist.co";

#[derive(clap::Subcommand)]
pub(crate) enum Command {
	/// Connect AniList through browser authentication.
	Login,
	/// Show the connected AniList identity without revealing its token.
	Status,
	/// Remove the saved AniList connection.
	Logout {
		/// Log out without asking for confirmation.
		#[arg(short, long)]
		yes: bool,
	},
	/// Show the connected account's read-only anime lists.
	List,
}

impl Command {
	pub(crate) const fn opens_tui(&self) -> bool {
		matches!(self, Self::Login | Self::List)
	}
}

#[derive(Clone)]
pub(crate) struct Client {
	http: HttpClient,
	token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct Viewer {
	pub id: u64,
	pub name: String,
	pub avatar: Option<Avatar>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct Avatar {
	pub medium: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Library {
	pub lists: Vec<ListGroup>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListGroup {
	pub name: String,
	#[serde(default)]
	pub is_custom_list: bool,
	pub status: Option<ListStatus>,
	#[serde(default)]
	pub entries: Vec<ListEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListEntry {
	pub id: u64,
	pub status: Option<ListStatus>,
	#[serde(default)]
	pub progress: u32,
	#[serde(default)]
	pub score: f64,
	#[serde(default)]
	pub priority: u8,
	pub media: Media,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Media {
	pub id: u64,
	pub id_mal: Option<u64>,
	pub is_adult: Option<bool>,
	pub title: MediaTitle,
	pub next_airing_episode: Option<NextAiring>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaTitle {
	pub user_preferred: Option<String>,
	pub romaji: Option<String>,
	pub english: Option<String>,
	pub native: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NextAiring {
	pub airing_at: i64,
	pub episode: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ListStatus {
	Current,
	Planning,
	Completed,
	Dropped,
	Paused,
	Repeating,
}

#[derive(Deserialize)]
struct GraphQlEnvelope<T> {
	data: Option<T>,
	#[serde(default)]
	errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
struct GraphQlError {
	message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ViewerData {
	viewer: Viewer,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LibraryData {
	media_list_collection: Library,
}

#[derive(Serialize)]
struct GraphQlRequest<'a, V> {
	query: &'a str,
	variables: V,
}

#[derive(Deserialize)]
struct Claims {
	exp: Option<i64>,
}

impl Client {
	pub(crate) fn public() -> Result<Self, Error> {
		Self::new(None)
	}

	pub(crate) fn connected() -> Result<Option<Self>, Error> {
		credentials::load()?
			.map(|token| Self::new(Some(token.value().to_owned())))
			.transpose()
	}

	fn authenticated(token: String) -> Result<Self, Error> {
		Self::new(Some(token))
	}

	fn new(token: Option<String>) -> Result<Self, Error> {
		let http = HttpClient::builder()
			.https_only(true)
			.connect_timeout(Duration::from_secs(10))
			.timeout(Duration::from_secs(30))
			.user_agent(concat!("a365/", env!("CARGO_PKG_VERSION")))
			.build()
			.map_err(|error| {
				Error::with_debug(
					"Could not initialize the AniList client.",
					error,
				)
			})?;
		Ok(Self { http, token })
	}

	pub(crate) async fn viewer(&self) -> Result<Viewer, Error> {
		const QUERY: &str =
			"query ViewerIdentity { Viewer { id name avatar { medium } } }";
		Ok(self
			.query::<ViewerData, _>(QUERY, serde_json::json!({}))
			.await?
			.viewer)
	}

	pub(crate) async fn library(
		&self,
		viewer_id: u64,
		adult: preferences::AdultContent,
	) -> Result<Library, Error> {
		const QUERY: &str = r#"query AnimeLibrary($userId: Int!) {
  MediaListCollection(userId: $userId, type: ANIME) {
    lists { name isCustomList status entries {
      id status progress score(format: POINT_100) priority
      media { id idMal isAdult title { userPreferred romaji english native }
        nextAiringEpisode { airingAt episode } }
    } }
  }
}"#;
		let mut library = self
			.query::<LibraryData, _>(
				QUERY,
				serde_json::json!({ "userId": viewer_id }),
			)
			.await?
			.media_list_collection;
		filter_adult(&mut library, adult);
		Ok(library)
	}

	pub(crate) async fn query<T, V>(
		&self,
		query: &str,
		variables: V,
	) -> Result<T, Error>
	where
		T: DeserializeOwned,
		V: Serialize,
	{
		if query.split_whitespace().any(|word| word == "mutation") {
			return Err(Error::new(
				"a365's AniList integration is read-only and refuses mutations.",
			));
		}
		let mut request = self
			.http
			.post(GRAPHQL_ENDPOINT)
			.json(&GraphQlRequest { query, variables });
		if let Some(token) = &self.token {
			request = request.bearer_auth(token);
		}
		let response = request.send().await.map_err(|error| {
			Error::with_debug("Could not reach AniList.", error.without_url())
		})?;
		let status = response.status();
		let envelope =
			response
				.json::<GraphQlEnvelope<T>>()
				.await
				.map_err(|error| {
					Error::with_debug(
						"AniList returned an unreadable response.",
						error,
					)
				})?;
		if !status.is_success() || !envelope.errors.is_empty() {
			let detail = envelope
				.errors
				.into_iter()
				.map(|error| error.message)
				.collect::<Vec<_>>()
				.join("; ");
			return Err(Error::new(if detail.is_empty() {
				format!("AniList returned HTTP {status}.")
			} else {
				format!("AniList rejected the read-only query: {detail}")
			}));
		}
		envelope
			.data
			.ok_or_else(|| Error::new("AniList returned no data."))
	}
}

fn filter_adult(library: &mut Library, adult: preferences::AdultContent) {
	if adult == preferences::AdultContent::Hidden {
		for list in &mut library.lists {
			list.entries
				.retain(|entry| entry.media.is_adult == Some(false));
		}
	}
}

impl MediaTitle {
	pub(crate) fn display(&self) -> &str {
		self.user_preferred
			.as_deref()
			.or(self.romaji.as_deref())
			.or(self.english.as_deref())
			.or(self.native.as_deref())
			.unwrap_or("Untitled anime")
	}
}

impl ListStatus {
	pub(crate) const fn name(self) -> &'static str {
		match self {
			Self::Current => "Current",
			Self::Planning => "Planning",
			Self::Completed => "Completed",
			Self::Dropped => "Dropped",
			Self::Paused => "Paused",
			Self::Repeating => "Repeating",
		}
	}
}

pub(crate) async fn run(command: &Command) -> Result<(), Error> {
	match command {
		Command::Login => login().await,
		Command::Status => status().await,
		Command::Logout { yes } => logout(*yes),
		Command::List => show_library().await,
	}
}

async fn login() -> Result<(), Error> {
	if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
		return Err(Error::new(
			"AniList browser login requires an interactive terminal. Use ANILIST_ACCESS_TOKEN for non-interactive access.",
		));
	}
	let grant = oauth::login().await?;
	let client = Client::authenticated(grant.token.clone())?;
	let viewer = client.viewer().await?;
	credentials::store(&grant.token)?;
	ui::success(format!("Connected AniList as {}", viewer.name));
	if let Some(expires_in) = grant.expires_in {
		ui::note(format!(
			"Authorization expires in {} days.",
			expires_in / 86_400
		));
	}
	Ok(())
}

pub(crate) async fn connect_library(
	adult: preferences::AdultContent,
) -> Result<(Viewer, Library), Error> {
	let grant = oauth::login_in_tui().await?;
	let client = Client::authenticated(grant.token.clone())?;
	let viewer = client.viewer().await?;
	let library = client.library(viewer.id, adult).await?;
	credentials::store(&grant.token)?;
	Ok((viewer, library))
}

async fn status() -> Result<(), Error> {
	let Some(token) = credentials::load()? else {
		ui::note("AniList is not connected. Run `a365 anilist login`.");
		return Ok(());
	};
	let viewer = Client::authenticated(token.value().to_owned())?
		.viewer()
		.await?;
	let origin = match token.origin {
		credentials::Origin::Environment => "ANILIST_ACCESS_TOKEN",
		credentials::Origin::SecureStore => secure_store_name(),
	};
	ui::success(format!("AniList connected as {} · {origin}", viewer.name));
	if let Some(expiry) = token_expiry(token.value()) {
		ui::note(format!("Token expiry: {expiry}"));
	}
	Ok(())
}

fn logout(preauthorized: bool) -> Result<(), Error> {
	if credentials::load()?
		.is_some_and(|token| token.origin == credentials::Origin::Environment)
	{
		return Err(Error::new(
			"AniList is connected through ANILIST_ACCESS_TOKEN. Unset that environment variable to log out.",
		));
	}
	if !preauthorized
		&& !ui::confirm(
			"Disconnect AniList and remove its saved token?",
			false,
		)? {
		ui::note("AniList logout cancelled.");
		return Ok(());
	}
	credentials::remove()?;
	ui::success("AniList disconnected");
	Ok(())
}

async fn show_library() -> Result<(), Error> {
	let client = Client::connected()?.ok_or_else(|| {
		Error::new("AniList is not connected. Run `a365 anilist login`.")
	})?;
	let viewer = client.viewer().await?;
	let preferences =
		preferences::Store::discover()?.load(Default::default())?;
	let adult = preferences.adult_content();
	let library = client.library(viewer.id, adult).await?;
	ui::heading(&format!("AniList · {}", viewer.name));
	for list in library.lists {
		println!("\n{}", list.name);
		for entry in list.entries {
			let status = entry.status.map_or("Unlisted", ListStatus::name);
			println!(
				"  {} · {status} · {}/? · score {:.0} · priority {}",
				entry.media.title.display(),
				entry.progress,
				entry.score,
				entry.priority,
			);
		}
	}
	Ok(())
}

pub(crate) fn remove_stored_token() -> Result<(), Error> {
	credentials::remove()
}

fn token_expiry(token: &str) -> Option<String> {
	let payload = token.split('.').nth(1)?;
	let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
	let expiry = serde_json::from_slice::<Claims>(&bytes).ok()?.exp?;
	chrono::DateTime::from_timestamp(expiry, 0)
		.map(|expiry| expiry.format("%Y-%m-%d %H:%M UTC").to_string())
}

const fn secure_store_name() -> &'static str {
	if cfg!(target_os = "macos") {
		"macOS Keychain"
	} else {
		"Linux Secret Service"
	}
}

#[cfg(test)]
#[path = "anilist_tests.rs"]
mod tests;
