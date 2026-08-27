use console::strip_ansi_codes;
use rapidfuzz::distance::osa::{Args, BatchComparator};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

#[derive(Debug)]
pub struct Search {
	rows: Vec<Vec<Vec<Token>>>,
}

#[derive(Debug)]
struct Token {
	text: String,
	len: usize,
	signature: u64,
}

impl Search {
	pub fn new<const N: usize>(rows: &[[String; N]]) -> Self {
		Self {
			rows: rows
				.iter()
				.map(|row| {
					row.iter()
						.map(|field| {
							normalize(field)
								.into_iter()
								.map(Token::new)
								.collect()
						})
						.collect()
				})
				.collect(),
		}
	}

	pub fn ranked(&self, query: &str) -> Vec<usize> {
		self.ranked_to(query, None)
	}

	pub fn ranked_limit(&self, query: &str, limit: usize) -> Vec<usize> {
		self.ranked_to(query, Some(limit))
	}

	fn ranked_to(&self, query: &str, limit: Option<usize>) -> Vec<usize> {
		let query = normalize(query);
		if query.is_empty() {
			return (0..limit
				.map_or(self.rows.len(), |limit| limit.min(self.rows.len())))
				.collect();
		}
		if limit == Some(0) {
			return Vec::new();
		}
		let query = query.iter().map(Query::new).collect::<Vec<_>>();
		let mut matches = self
			.rows
			.iter()
			.enumerate()
			.filter_map(|(index, row)| {
				let score = query.iter().try_fold(0_u32, |score, query| {
					best_score(row, query)
						.map(|matched| score.saturating_add(matched))
				})?;
				Some((index, score))
			})
			.collect::<Vec<_>>();
		if let Some(limit) = limit
			&& limit < matches.len()
		{
			matches.select_nth_unstable_by(limit, compare_matches);
			matches.truncate(limit);
		}
		matches.sort_unstable_by(compare_matches);
		matches.into_iter().map(|(index, _)| index).collect()
	}

	pub fn len(&self) -> usize {
		self.rows.len()
	}
}

fn compare_matches(
	left: &(usize, u32),
	right: &(usize, u32),
) -> std::cmp::Ordering {
	right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
}

struct Query<'a> {
	text: &'a str,
	len: usize,
	budget: usize,
	signature: u64,
	comparator: BatchComparator<char>,
}

impl<'a> Query<'a> {
	fn new(text: &'a String) -> Self {
		let len = text.chars().count();
		Self {
			text,
			len,
			budget: typo_budget(len),
			signature: signature(text),
			comparator: BatchComparator::new(text.chars()),
		}
	}
}

impl Token {
	fn new(text: String) -> Self {
		let len = text.chars().count();
		let signature = signature(&text);
		Self {
			text,
			len,
			signature,
		}
	}

	fn prefix(&self, len: usize) -> &str {
		if len >= self.len {
			&self.text
		} else {
			let end = self
				.text
				.char_indices()
				.nth(len)
				.map_or(self.text.len(), |(index, _)| index);
			&self.text[..end]
		}
	}
}

fn best_score(row: &[Vec<Token>], query: &Query<'_>) -> Option<u32> {
	row.iter()
		.enumerate()
		.flat_map(|(field, tokens)| {
			let field = u32::try_from(row.len() - field).unwrap_or(u32::MAX);
			tokens
				.iter()
				.filter_map(move |token| token_score(token, query))
				.map(move |score| score.saturating_add(field * 10))
		})
		.max()
}

fn token_score(token: &Token, query: &Query<'_>) -> Option<u32> {
	if token.text == query.text {
		return Some(4_000);
	}
	if token.text.starts_with(query.text) {
		return Some(3_000);
	}
	if token.text.contains(query.text) {
		return Some(2_500);
	}
	if query.budget == 0 {
		return None;
	}
	if (token.signature & query.signature)
		.count_ones()
		.saturating_add(u32::try_from(query.budget).unwrap_or(u32::MAX))
		< query.signature.count_ones()
	{
		return None;
	}
	let distance = query.comparator.distance_with_args(
		token.prefix(query.len.saturating_add(query.budget)).chars(),
		&Args::default().score_cutoff(query.budget),
	)?;
	Some(
		2_000_u32.saturating_sub(
			u32::try_from(distance)
				.unwrap_or(u32::MAX)
				.saturating_mul(200),
		),
	)
}

fn signature(input: &str) -> u64 {
	input.chars().fold(0, |signature, character| {
		signature | 1 << (u32::from(character) % 64)
	})
}

pub(crate) fn typo_budget(len: usize) -> usize {
	match len {
		0..=2 => 0,
		3..=5 => 1,
		_ => (len / 5).max(2),
	}
}

fn normalize(input: &str) -> Vec<String> {
	let input = strip_ansi_codes(input);
	let mut normalized = String::with_capacity(input.len());
	for character in input
		.nfd()
		.filter(|character| !is_combining_mark(*character))
		.flat_map(char::to_lowercase)
	{
		normalized.push(if character.is_alphanumeric() {
			character
		} else {
			' '
		});
	}
	normalized
		.split_whitespace()
		.map(ToOwned::to_owned)
		.collect()
}

pub(crate) fn normalize_query(input: &str) -> String {
	normalize(input).join(" ")
}
