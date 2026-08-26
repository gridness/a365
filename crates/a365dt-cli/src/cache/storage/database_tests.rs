use std::{fs, process, time::SystemTime};

use super::{open, rebuild};

#[tokio::test]
async fn rebuilds_after_a_shared_lock_was_inherited() {
	let directory = std::env::temp_dir().join(format!(
		"a365-cache-inherited-lock-{}-{}",
		process::id(),
		SystemTime::now()
			.duration_since(SystemTime::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	));
	let database = open(&directory)
		.await
		.unwrap_or_else(|failure| panic!("{}", failure.error.render(true)));
	let inherited_lock = database._lock.0.try_clone().unwrap();
	database.pool.close().await;
	drop(database);

	rebuild(&directory).await.unwrap();

	drop(inherited_lock);
	fs::remove_dir_all(directory).unwrap();
}
