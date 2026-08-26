#!/usr/bin/env bash

set -euo pipefail

private_key=""
client_id=""
source_repository=""
tap_repository="Gridness/homebrew-oosama"
confirmed=false

while (( $# )); do
	case "$1" in
		--private-key)
			private_key=${2:-}
			shift 2
			;;
		--client-id)
			client_id=${2:-}
			shift 2
			;;
		--source)
			source_repository=${2:-}
			shift 2
			;;
		--tap)
			tap_repository=${2:-}
			shift 2
			;;
		--yes)
			confirmed=true
			shift
			;;
		*)
			printf 'Unknown argument: %s\n' "$1" >&2
			exit 2
			;;
	esac
done

if [[ -z "$private_key" || -z "$client_id" ]]; then
	printf 'Usage: %s --private-key PATH --client-id ID [--source OWNER/REPO] [--tap OWNER/REPO] --yes\n' "$0" >&2
	exit 2
fi
[[ -f "$private_key" ]] || {
	printf 'Private key does not exist: %s\n' "$private_key" >&2
	exit 2
}
[[ "$confirmed" == true ]] || {
	printf 'Refusing the remote write dry run without --yes.\n' >&2
	exit 2
}

for command in curl git gh jq openssl; do
	command -v "$command" >/dev/null 2>&1 || {
		printf 'Required command is unavailable: %s\n' "$command" >&2
		exit 2
	}
done

if [[ -z "$source_repository" ]]; then
	source_repository=$(gh repo view --json nameWithOwner --jq '.nameWithOwner' 2>/dev/null || printf '%s' 'Gridness/a365')
fi

api=https://api.github.com
temporary_directory=$(mktemp -d)
source_checkout="$temporary_directory/source"
tap_checkout="$temporary_directory/tap"
askpass="$temporary_directory/askpass.sh"
source_token=""
tap_token=""
source_branch="a365-release-app-dry-run-$(date +%s)-$$"
tap_branch="$source_branch"
source_branch_pushed=false
tap_branch_pushed=false
pull_request_number=""

printf '%s\n' \
	'#!/usr/bin/env bash' \
	'case "$1" in' \
	'  *Username*) printf "%s\\n" "x-access-token" ;;' \
	'  *) printf "%s\\n" "$A365_RELEASE_APP_TOKEN" ;;' \
	'esac' > "$askpass"
chmod 700 "$askpass"

git_as_app() {
	local token=$1
	shift
	A365_RELEASE_APP_TOKEN=$token \
		GIT_ASKPASS=$askpass \
		GIT_TERMINAL_PROMPT=0 \
		git "$@"
}

api_as_app() {
	local method=$1 path=$2 data=${3:-}
	local -a arguments=(
		--fail
		--silent
		--show-error
		--request "$method"
		--header @-
		"$api$path"
	)
	if [[ -n "$data" ]]; then
		arguments+=(--data "$data")
	fi
	printf '%s\n' \
		"Authorization: Bearer $jwt" \
		'Accept: application/vnd.github+json' \
		'Content-Type: application/json' \
		'X-GitHub-Api-Version: 2022-11-28' |
		curl "${arguments[@]}"
}

api_as_installation() {
	local token=$1 method=$2 path=$3 data=${4:-}
	if [[ -n "$data" ]]; then
		GH_TOKEN=$token gh api --method "$method" "$path" --input - <<<"$data"
	else
		GH_TOKEN=$token gh api --method "$method" "$path"
	fi
}

revoke_token() {
	local token=$1
	[[ -n "$token" ]] || return 0
	GH_TOKEN=$token gh api --method DELETE /installation/token >/dev/null 2>&1 || true
}

cleanup() {
	local status=$?
	trap - EXIT INT TERM
	set +e
	if [[ -n "$pull_request_number" && -n "$source_token" ]]; then
		api_as_installation \
			"$source_token" PATCH "/repos/$source_repository/pulls/$pull_request_number" \
			'{"state":"closed"}' >/dev/null
	fi
	if [[ "$source_branch_pushed" == true && -n "$source_token" ]]; then
		git_as_app "$source_token" -C "$source_checkout" push --quiet origin \
			--delete "$source_branch" >/dev/null 2>&1
	fi
	if [[ "$tap_branch_pushed" == true && -n "$tap_token" ]]; then
		git_as_app "$tap_token" -C "$tap_checkout" push --quiet origin \
			--delete "$tap_branch" >/dev/null 2>&1
	fi
	revoke_token "$source_token"
	revoke_token "$tap_token"
	unset jwt source_token tap_token A365_RELEASE_APP_TOKEN
	rm -rf "$temporary_directory"
	exit "$status"
}
trap cleanup EXIT INT TERM

base64url() {
	openssl base64 -A | tr '+/' '-_' | tr -d '='
}

now=$(date +%s)
header=$(printf '%s' '{"alg":"RS256","typ":"JWT"}' | base64url)
payload=$(printf '{"iat":%s,"exp":%s,"iss":"%s"}' \
	"$((now - 60))" "$((now + 540))" "$client_id" | base64url)
unsigned="$header.$payload"
signature=$(printf '%s' "$unsigned" |
	openssl dgst -sha256 -sign "$private_key" | base64url)
jwt="$unsigned.$signature"
unset signature unsigned

app=$(api_as_app GET /app)
actual_client_id=$(jq -r '.client_id' <<<"$app")
[[ "$actual_client_id" == "$client_id" ]] || {
	printf 'The private key belongs to client %s, not %s.\n' \
		"$actual_client_id" "$client_id" >&2
	exit 1
}

source_owner=${source_repository%%/*}
installations=$(api_as_app GET /app/installations)
installation_id=$(jq -r --arg owner "$source_owner" \
	'.[] | select((.account.login | ascii_downcase) == ($owner | ascii_downcase)) | .id' \
	<<<"$installations")
[[ -n "$installation_id" ]] || {
	printf 'No GitHub App installation was found for %s.\n' "$source_owner" >&2
	exit 1
}

source_name=${source_repository#*/}
source_request=$(jq -nc --arg repository "$source_name" '{
	repository_names: [$repository],
	permissions: {contents: "write", issues: "write", pull_requests: "write"}
}')
source_response=$(api_as_app POST \
	"/app/installations/$installation_id/access_tokens" "$source_request")
source_token=$(jq -r '.token' <<<"$source_response")
[[ -n "$source_token" && "$source_token" != null ]] || {
	printf 'GitHub did not issue the source-repository installation token.\n' >&2
	exit 1
}
unset source_response source_request

printf 'Verifying source release-PR create/update with a temporary draft…\n'
source_clone_url=$(api_as_installation "$source_token" GET \
	"/repos/$source_repository" | jq -r '.clone_url')
git_as_app "$source_token" clone --quiet --depth=1 \
	"$source_clone_url" "$source_checkout"
git -C "$source_checkout" switch --quiet -c "$source_branch"
git -C "$source_checkout" config user.name 'a365 release App dry run'
git -C "$source_checkout" config user.email '41898282+github-actions[bot]@users.noreply.github.com'
mkdir -p "$source_checkout/.github"
printf 'a365 release App dry run: create\n' > \
	"$source_checkout/.github/a365-release-app-dry-run.txt"
git -C "$source_checkout" add .github/a365-release-app-dry-run.txt
git -C "$source_checkout" commit --quiet -m 'test: verify release App PR creation'
source_branch_pushed=true
git_as_app "$source_token" -C "$source_checkout" push --quiet origin \
	"HEAD:refs/heads/$source_branch"

pull_request=$(jq -nc \
	--arg title 'test: verify a365 release GitHub App' \
	--arg head "$source_branch" \
	--arg body 'Automated reversible write dry run for issue #108. This draft will be closed and its branch deleted.' \
	'{title: $title, head: $head, base: "main", body: $body, draft: true}')
pull_request_response=$(api_as_installation "$source_token" POST \
	"/repos/$source_repository/pulls" "$pull_request")
pull_request_number=$(jq -r '.number' <<<"$pull_request_response")
[[ -n "$pull_request_number" && "$pull_request_number" != null ]] || {
	printf 'GitHub did not create the release-App dry-run pull request.\n' >&2
	exit 1
}

printf 'a365 release App dry run: update\n' > \
	"$source_checkout/.github/a365-release-app-dry-run.txt"
git -C "$source_checkout" add .github/a365-release-app-dry-run.txt
git -C "$source_checkout" commit --quiet -m 'test: verify release App PR update'
git_as_app "$source_token" -C "$source_checkout" push --quiet origin \
	"HEAD:refs/heads/$source_branch"
expected_head=$(git -C "$source_checkout" rev-parse HEAD)
actual_head=""
for attempt in 1 2 3 4 5 6 7 8 9 10; do
	actual_head=$(api_as_installation "$source_token" GET \
		"/repos/$source_repository/pulls/$pull_request_number" | jq -r '.head.sha')
	[[ "$actual_head" == "$expected_head" ]] && break
	sleep 1
done
[[ "$actual_head" == "$expected_head" ]] || {
	printf 'The dry-run pull request did not receive the pushed update.\n' >&2
	exit 1
}

tap_name=${tap_repository#*/}
tap_request=$(jq -nc --arg repository "$tap_name" '{
	repository_names: [$repository],
	permissions: {contents: "write"}
}')
tap_response=$(api_as_app POST \
	"/app/installations/$installation_id/access_tokens" "$tap_request")
tap_token=$(jq -r '.token' <<<"$tap_response")
[[ -n "$tap_token" && "$tap_token" != null ]] || {
	printf 'GitHub did not issue the tap installation token.\n' >&2
	exit 1
}
unset tap_response tap_request jwt

printf 'Verifying independent Homebrew tap checkout/push…\n'
tap_clone_url=$(api_as_installation "$tap_token" GET \
	"/repos/$tap_repository" | jq -r '.clone_url')
git_as_app "$tap_token" clone --quiet --depth=1 \
	"$tap_clone_url" "$tap_checkout"
git -C "$tap_checkout" switch --quiet -c "$tap_branch"
git -C "$tap_checkout" config user.name 'a365 release App dry run'
git -C "$tap_checkout" config user.email '41898282+github-actions[bot]@users.noreply.github.com'
mkdir -p "$tap_checkout/.github"
printf 'a365 Homebrew App dry run\n' > \
	"$tap_checkout/.github/a365-release-app-dry-run.txt"
git -C "$tap_checkout" add .github/a365-release-app-dry-run.txt
git -C "$tap_checkout" commit --quiet -m 'test: verify release App tap push'
tap_branch_pushed=true
git_as_app "$tap_token" -C "$tap_checkout" push --quiet origin \
	"HEAD:refs/heads/$tap_branch"

printf 'Verified: source draft PR #%s was created and updated.\n' \
	"$pull_request_number"
printf 'Verified: the tap accepted an independently scoped checkout/push.\n'
printf 'Cleaning up both branches, closing the draft PR, and revoking both tokens…\n'
