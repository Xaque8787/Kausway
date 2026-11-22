# Manual release publication

Pushes to every branch, including main, run **Build Validation** and create testable
artifacts. They do not publish releases. Pushing a Git tag does not publish either.
Only **Publish release**, manually dispatched on `main`, publishes selected tracks.
Existing `v0.4.0` releases and their tags/assets are preserved.

## Prepare on a development branch

1. Edit `releases.toml` for the tracks being released. Suite and client versions
   are independent. Container has its own version and explicit `suite_version`.
   Publication verifies strict SemVer advancement; it never edits version files
   or creates a follow-up version-bump commit.
2. Run `python3 scripts/releases/sync-versions.py`, review its changes, then run
   `python3 scripts/releases/sync-versions.py --check`. Commit the manifest and all
   generated version changes together. See [versioning](versioning.md).
3. Push the branch. Wait for **Build Validation**, including its **Validation
   summary**, to succeed. Download artifacts for practical tests; their existence
   alone is not proof the run passed. See [artifact instructions](ci.md).
4. Merge to `main` and wait for a successful **Build Validation** run for the
   resulting main commit. Branch/PR results do not substitute for this run.
5. In **Actions → Publish release → Run workflow**, select `main`, choose suite,
   client, container, or any combination, then dispatch. Select at least one.

The workflow must first exist on the default branch for GitHub to expose the
manual launcher. Do not run the retired Binaries/container publishers from an old
commit or branch. They are removed from the current workflow model.

The initial manifest values do not represent new releases: suite/client `0.4.0`
will fail advancement against the existing `v0.4.0` release. Bump them before the
first new release. The first container release also needs a published **new-layout**
suite matching `container.suite_version`. Select suite and container together, or
publish that suite first. Legacy combined archives are not substituted.

## What the launcher pins and verifies

GitHub captures a full commit SHA when dispatching on main. Every job checks out
that SHA explicitly, including after queuing or rerunning. The run summary records
the SHA, selected versions, validation run/attempt and existing release states.
It never silently switches to a newer main head. The candidate must remain an
ancestor of main; subsequent unrelated main commits are allowed.

Preflight and subsequent publication operations query GitHub again. They require
the latest applicable **Build Validation** run for that exact SHA to be complete
and successful, from this repository, on main, triggered by a push/manual validation.
An older green result cannot override a newer pending, failed or canceled run.
The expected workflow path is `.github/workflows/build-validation.yml`.

The publisher verifies the **Validation summary** job and its GitHub Actions check,
then downloads its matching run/attempt report. That report must match the explicit
policy in `scripts/ci/validation-policy.json`; every required job must succeed.
There are currently no legitimate skip exceptions. A failed-jobs rerun may reuse
successful artifacts/summary jobs from earlier attempts of the same run, with their
original identities preserved. Missing, expired, ambiguous or dirty-source evidence
fails closed. If a newer validation run/attempt is selected after preflight, start
the publisher again to select that evidence explicitly.

The release plan freezes candidate manifest hashes and the validation run/attempt.
Plan and platform-receipt artifact names include SHA, publisher run ID and attempt.
Local dirty-source overrides are forbidden in Actions.

## Releases and images produced

| Selection | Published identity | Contents |
| --- | --- | --- |
| Suite | `suite-v<version>` GitHub Release/tag | One release page for every supported CPU; Linux combined suite and standalone manager/host/admin archives, Android standalone host archives, stable-only Jellyfin plugin |
| Client | `client-v<version>` GitHub Release/tag | CLI archives for supported CPUs and Android SDK AAR/POM/sources/notices/README |
| Container | `container-v<version>` GitHub Release/tag | Checked container descriptor recording source, selected suite and registry digests; GHCR image/index |

Each GitHub Release includes adjacent checksums, `SHA256SUMS` and
`RELEASE-MANIFEST.json`. Suite/client publication promotes the exact verified
candidate files from the successful main run, without rebuilding them. A page
contains all architectures; architecture is an asset selection, not a separate
release. See [complete asset policy](release-packaging.md).

Suite prereleases, such as `0.5.0-rc.1`, still build/test Jellyfin but do not
distribute its plugin. Jellyfin's numeric four-part version cannot distinguish
that prerelease from `0.5.0`. Stable suite releases require the plugin asset.
Client and container prereleases have their own independent classifications.

The container publisher downloads and verifies the selected published suite's
complete release inventory. Native AMD64 and ARM64 jobs copy its exact three Linux
binaries and notices into the runtime image. Suite and container source commits
may differ and are recorded separately, with suite asset/manifest hashes.

Buildx exports each platform once as OCI bytes, with attestations disabled so the
final index has exactly two platform manifests. The test loader verifies and converts
the archive to Docker's classic load format without changing the config or layer
contents. Runtime tests check the binary hashes/metadata, setup/login, lifecycle,
persistence and Alpine/Ubuntu `COPY --from` consumers. Only after these pass are the
original OCI bytes uploaded by digest. The image is never rebuilt per release tag.

After both platforms pass, one verified index is published at
`ghcr.io/xaque8787/kausway`, with identical index digests for:

- `<container-version>` — immutable release identity.
- `sha-<full-container-source-commit>` — immutable source identity.
- `latest` — advanced only after a stable container release completes.

`latest` tracks the most recent **published stable container release**, not main
or a development branch. Prereleases receive version and SHA tags only. Automatic
development builds provide downloadable image artifacts instead of GHCR tags.
The descriptor's `tags` records the release's intended promotion; an older
descriptor does not assert that `latest` still points there today.

## Independence, serialization and permissions

Suite and client publication jobs run independently after preflight. Container
waits for suite publication only when that selected suite is being published in
the same run. An unrelated client failure, or failure of a different suite version,
does not prevent a valid container track from finishing. The overall run still
reports any failure; successful tracks remain published.

One workflow-wide concurrency group serializes publishers, with
`cancel-in-progress: false`. GitHub keeps one running and at most one pending run;
this is not a FIFO queue. Review canceled/superseded dispatches explicitly. Every
remote mutation rechecks relevant main/tag/release state rather than relying only
on preflight decisions.

The workflow uses the default repository `GITHUB_TOKEN`, scoped per job to read
Actions/checks and write only the needed releases/packages. No PAT or additional
environment-approval gate is required by this design. Ensure repository/org Actions
settings permit these permissions and GHCR package access. New GHCR packages may
need their visibility changed to public before anonymous pulls work.

Tags created with the default token do not trigger other Actions workflows under
GitHub's normal token rules. Tag publication triggers are also removed, covering
manually pushed tags and future credential changes. Do not add a PAT to make
release tags trigger another publisher.

## Interrupted publication and retries

Publication distinguishes **not published**, **incomplete**, and **complete**:

- A new track creates its immutable tag and a draft GitHub Release. It uploads
  files, downloads/verifies the entire expected inventory, then publishes the draft.
- An incomplete draft can resume only when every existing asset exactly matches
  the validated candidate. Identical assets are retained; missing assets are added.
  Unexpected, unfinished or conflicting assets stop the run for inspection.
- A complete release is verified and reported as already complete. Its assets and
  immutable Git/image tags are never silently replaced.

Prefer **Re-run failed jobs** on the original publisher run, preserving its plan,
selected source and platform receipts. Platform receipt artifacts survive separate
attempts; final assembly can reuse a successful platform from an earlier attempt.
If version tags were already written, matching original tested receipts are required
to finish their index. Rebuilding different image bytes cannot overwrite those tags.

A new dispatch can verify completed tracks or repair stable `latest` promotion
after a container release completed. Resuming an incomplete image under a new run
may fail because it lacks the original receipts or produces different bytes. If
evidence expired, rerun validation and/or prepare a new patch release; do not bypass
verification. Inspect failed upload placeholders/conflicting drafts manually. Clean
up only positively identified unfinished work; never delete/replace completed assets
or repoint version/source tags to force a retry through.

If `latest` promotion fails after the container GitHub Release is complete, the
run fails visibly. Rerunning verifies the existing release/index/tags and repairs
the pointer without rebuilding. Rerunning an older completed version after a newer
stable release does not move `latest` backward. Suite/client do not roll back when
another track fails.

## A defective published release

Publish a corrected patch version for the affected track. Preserve the defective
release's immutable version tags, assets and audit identity; explain the defect
in its release notes and point users to the fixed version. Do not reuse its version.
For urgent container mitigation, a maintainer can deliberately restore `latest`
to the previous known-good **index digest** recorded in its container descriptor,
using authenticated registry tooling that preserves that manifest's digest. Verify
the remote digest and both platforms afterwards. A SHA tag is not the rollback
record: registry tags can be changed, whereas content digests identify exact bytes.

This is a documented operator action, not an automatic rollback workflow. Check
storage/config compatibility before running older binaries against persistent
state, and retain backups. Rerunning the defective release is not a rollback tool;
it may restore its own `latest` pointer if no newer stable version exists.

## Practical acceptance record (guidance, not a workflow gate)

Before publication, use candidate artifacts from the **exact SHA intended for
publication**, and record SHA/run/attempts and results in the release PR or notes.
If the main merge produced another SHA, record that difference and test its main
artifacts as appropriate; do not label an earlier branch build as the published one.

- Standalone suite starts with expected default/explicit storage paths.
- Admin first-run setup, sign-in and manager-backed host controls work.
- Phone pairing, media-server login, playback and seeking work.
- Wi-Fi/mobile-data operation works.
- Sidecar reaches the origin through Docker networking.
- Container recreation retains credentials, identity and pairing.
- Standalone state can be reused through correctly configured bind mounts.

After publication, use the **actual released executables/images** for a shorter
check: downloads/checksums, container pull/start, downstream `COPY --from`, and at
least one real client pairing/login/playback path. Fresh-install setup and reused
state are complementary scenarios. Verify executable versions and image digests
so a still-running development build does not masquerade as released software.

These checks are release-guide conventions, not an enforced approval or form in
the publisher. CI covers its automated tests; green CI does not claim real-phone
and real-network acceptance testing occurred. The remaining Phase 8 documentation
audit and post-publication link audit should point users to items that actually exist.

## Local verification of the publisher

```sh
python3 scripts/releases/sync-versions.py --check
python3 -m unittest discover -s scripts/releases -p 'test_*.py'
python3 -m unittest discover -s scripts/ci -p 'test_*.py'
actionlint .github/workflows/*.yml
```

Unit tests use in-memory GitHub/registry boundaries, including interrupted asset
upload, conflicting immutable identities, prereleases, partial image publication
and failed `latest` promotion. They never contact real publication endpoints.
Actual Actions permissions, GHCR upload behavior and native ARM64 execution still
need confirmation from GitHub when the manual workflow is available on main.
