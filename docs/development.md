# Development

## Repository layout

- `package/*/Makefile`: package sources, versions, library names, and features.
- `package/rust-library.mk`: shared Rust compilation and installation rules.
- `.github/scripts/run-sdk.sh` and `.github/scripts/build-sdk.sh`: SDK setup, compilation, and ARM64 load checks.
- `.github/scripts/publish-apks.sh`: latest-only APK publishing.
- `.github/scripts/update-versions.py`: upstream checks and version updates.

## Automatic updates

Add the repository Actions secret `UPSTREAM_UPDATE_TOKEN`, using a token with
**Contents** and **Workflows** read/write permissions for this repository.
Install the updater workflow on `main`.

**Update upstream versions** checks every Monday at 02:00 UTC. Manual runs check immediately
and default to preview.
It follows stable package releases and OpenWrt patch releases within the existing
release series. Package versions, source hashes, and dependencies live in each
package's `Makefile` and `files/Cargo.lock`. The exact candidate must pass
the OpenWrt SDK build and APK tests before the workflow pushes it to `main`.
A source or compiler incompatibility stops the update for review.

New Tailscale Rust releases are pinned to their release commit and source hash.
The updater qualifies the exact candidate and leaves it as a patch artifact for
a pull request. Review the upstream changelog and C API, update the package documentation
when the exposed features change, and run the SDK checks on the PR.

The updater can propose compatibility changes for both packages using the
pinned hosted inference tool. Configure the repository secret referenced in the
[updater workflow](../.github/workflows/update-versions.yml) to enable this step.
The workflow verifies the session's model
identity before accepting edits. To test inference on an isolated text fixture,
run **Update upstream versions** with preview enabled and select the option
labeled **Verify the locked CLI and model on an isolated text fixture only**.

Run updater regression tests with
`python3 -m unittest discover -s .github/scripts -p 'test_*.py'`.

## Maintenance preview

Run **Preview OpenWrt maintenance** manually in GitHub Actions. Performance,
refactor, and cleanup produce separate review artifacts. Performance proposals
use paired SDK builds with isolated cold and warm caches, or compare APK size.
A new benchmark is a review artifact. Qualified proposals can be applied and
reviewed in a normal pull request.

To exercise upstream qualification without waiting for a release, run
**Update upstream versions** with preview enabled and select a package under
**replay**. This checks the package at its recorded upstream revision and source
hash. For Tailscale release updates, review the agent's proposed packaging
adaptations and findings alongside the SDK result and C interface checks.
