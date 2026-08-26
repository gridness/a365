# Rename the public product to a365

Version 3 renames the complete public product from a365dt to a365, including its executable, repository, Homebrew formula, release artifacts, documentation, application homes, credentials, completions, and update surfaces. Existing application state and credentials migrate automatically, and an `a365dt` compatibility executable remains for one major release; the internal `a365dt-cli` crate name remains initially to preserve the repository's crate convention and avoid coupling public identity to internal packaging.
