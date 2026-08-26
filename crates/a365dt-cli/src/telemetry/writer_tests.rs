use std::{
	fs, process,
	time::{Duration, SystemTime},
};

use pretty_assertions::assert_eq;
use tokio::sync::{mpsc, oneshot};

use super::{receive_batch, remember_failure};
use crate::content::ContentSource;
use crate::telemetry::{
	Command, CommandOutcome, InvocationId, Paths,
	recording::{Observation, ObservationKind, SeriesIdentity},
	snapshot,
	storage::Store,
};

#[tokio::test(start_paused = true)]
async fn batches_for_one_second_and_finishing_drains_the_tail() {
	let invocation_id = InvocationId::new();
	let (observations, mut receiver) = mpsc::unbounded_channel();
	let (finish, mut finishing) = oneshot::channel();
	observations
		.send(command_observation(invocation_id, 1, Command::Download))
		.unwrap();
	let started = tokio::time::Instant::now();
	let first_batch = {
		let batch = receive_batch(&mut receiver, &mut finishing);
		tokio::pin!(batch);
		assert!(
			tokio::time::timeout(Duration::from_millis(999), batch.as_mut())
				.await
				.is_err()
		);
		batch.await
	};
	assert_eq!(
		(first_batch, started.elapsed()),
		(
			Some((
				vec![command_observation(invocation_id, 1, Command::Download)],
				false,
			)),
			Duration::from_secs(1),
		)
	);
	observations
		.send(command_observation(invocation_id, 2, Command::Update))
		.unwrap();
	observations
		.send(command_observation(invocation_id, 3, Command::Download))
		.unwrap();
	finish.send(()).unwrap();
	assert_eq!(
		receive_batch(&mut receiver, &mut finishing).await,
		Some((
			vec![
				command_observation(invocation_id, 2, Command::Update),
				command_observation(invocation_id, 3, Command::Download),
			],
			true,
		))
	);
}

#[tokio::test]
async fn remembers_the_first_failure_and_keeps_committing() {
	let paths = paths();
	let store = Store::open(paths.clone()).await.unwrap();
	let invocation_id = InvocationId::new();
	let mut watermark = None;
	let mut first_error = None;
	remember_failure(
		store
			.commit(
				&mut watermark,
				vec![Observation {
					invocation_id,
					observed_at_ms: 1,
					kind: ObservationKind::SeriesSelection {
						identity: SeriesIdentity::Included {
							source: ContentSource::Anime365,
							id: 0,
							title: "invalid".into(),
						},
						catalogue: None,
					},
				}],
			)
			.await,
		&mut first_error,
	);
	remember_failure(
		store
			.commit(
				&mut watermark,
				vec![Observation {
					invocation_id,
					observed_at_ms: 2,
					kind: ObservationKind::Command {
						command: Command::Update,
						outcome: CommandOutcome::Success,
					},
				}],
			)
			.await,
		&mut first_error,
	);

	assert_eq!(
		(
			first_error.unwrap().to_string(),
			snapshot::capture(&store).await.unwrap().counters,
		),
		(
			"Could not update the local telemetry. Close other a365 processes and retry."
				.into(),
			std::collections::BTreeMap::from([(
				"commands.update.success".into(),
				1,
			)]),
		)
	);
	store.close().await;
	fs::remove_dir_all(paths.data.parent().unwrap().parent().unwrap()).unwrap();
}

fn command_observation(
	invocation_id: InvocationId,
	observed_at_ms: u64,
	command: Command,
) -> Observation {
	Observation {
		invocation_id,
		observed_at_ms,
		kind: ObservationKind::Command {
			command,
			outcome: CommandOutcome::Success,
		},
	}
}

fn paths() -> Paths {
	let root = std::env::temp_dir().join(format!(
		"a365-telemetry-writer-error-{}-{}",
		process::id(),
		SystemTime::now()
			.duration_since(SystemTime::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	));
	Paths {
		data: root.join("data/telemetry.sqlite"),
		lock: root.join("data/telemetry.lock"),
		disabled: root.join("config/telemetry-disabled"),
	}
}
