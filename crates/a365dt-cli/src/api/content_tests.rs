use pretty_assertions::assert_eq;

use super::{AdultClassification, SeriesContent};

#[test]
fn series_content_classification_uses_hentai_status_fail_closed() {
	let classification = |fixture| {
		serde_json::from_str::<SeriesContent>(fixture)
			.unwrap()
			.classification()
	};

	assert_eq!(
		[
			classification(r#"{"isHentai":1}"#),
			classification(r#"{"isHentai":0}"#),
			classification(r#"{"isHentai":2}"#),
			classification("{}"),
		],
		[
			AdultClassification::Adult,
			AdultClassification::NonAdult,
			AdultClassification::Unknown,
			AdultClassification::Unknown,
		],
	);
}
