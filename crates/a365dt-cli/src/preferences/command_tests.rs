use pretty_assertions::assert_eq;

use super::{AdultOptInDecision, adult_opt_in_decision};
use crate::{api::AccessFailure, error::Error};

#[test]
fn adult_opt_in_distinguishes_success_denial_and_transient_failure() {
	let denied = Error::new("H365 denied access");
	let unavailable = Error::new("H365 temporarily unavailable");

	assert_eq!(
		[
			adult_opt_in_decision(Ok(())),
			adult_opt_in_decision(Err(AccessFailure::Denied(denied.clone()))),
			adult_opt_in_decision(Err(AccessFailure::Unavailable(
				unavailable.clone(),
			))),
		],
		[
			AdultOptInDecision::Enable,
			AdultOptInDecision::Refuse(denied),
			AdultOptInDecision::ConfirmTransient(unavailable),
		],
	);
}
