use pretty_assertions::assert_eq;

use crate::search::Search;

#[test]
fn ranks_order_independent_typo_matches_across_fields() {
	let rows = [
		[
			"Врата Штейна / Steins;Gate".into(),
			"2011".into(),
			"TV".into(),
		],
		["An unrelated gate".into(), "2024".into(), "Movie".into()],
		["Café".into(), "2020".into(), "Special".into()],
	];
	let search = Search::new(&rows);

	assert_eq!(
		[
			search.ranked("stiens gate"),
			search.ranked("stens gate"),
			search.ranked("steiins gate"),
			search.ranked("steans gate"),
			search.ranked("gate steins"),
			search.ranked("cafe"),
			search.ranked("steins missing"),
		],
		[vec![0], vec![0], vec![0], vec![0], vec![0], vec![2], vec![],]
	);
}

#[test]
fn weights_earlier_fields_and_preserves_order_for_ties() {
	let rows = [
		["Alpha".into(), "2024".into()],
		["2024".into(), "Alpha".into()],
		["2024".into(), "Beta".into()],
	];

	assert_eq!(Search::new(&rows).ranked("2024"), vec![1, 2, 0]);
}

#[test]
fn limited_ranking_is_the_prefix_of_the_complete_ranking() {
	let rows = (0..500)
		.map(|index| [format!("Series {index}"), format!("Alias {index}")])
		.collect::<Vec<_>>();
	let search = Search::new(&rows);

	assert_eq!(
		search.ranked_limit("series", 200),
		search.ranked("series")[..200],
	);
}
